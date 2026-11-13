package com.longbridge.nsh;

import android.app.DownloadManager;
import android.content.Context;
import android.net.Uri;
import android.os.Environment;
import android.util.Log;
import android.webkit.WebResourceRequest;
import android.webkit.WebResourceResponse;
import android.webkit.WebSettings;
import androidx.webkit.WebResourceResponseCompat;
import androidx.webkit.WebSettingsCompat;
import androidx.webkit.WebViewFeature;
import java.io.ByteArrayInputStream;
import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.HttpURLConnection;
import java.net.URL;
import java.nio.charset.Charset;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.regex.Pattern;
import java.util.zip.GZIPInputStream;

/**
 * Web view downloads on Android's system WebView, as a transport only (the
 * core decides states, destinations and cleanup).
 *
 * <p>Everything the page downloads outside {@code download_scope} ends in
 * {@link Client}'s {@code DownloadListener} with the response already gone
 * (Chromium discarded it): that is a {@code None} offer, which the app can
 * only acknowledge or — when the request is provably a GET — delegate. A
 * URL's method is never inferred from its shape.
 *
 * <p>Inside {@code download_scope} every GET is ours: {@code
 * shouldInterceptRequest} issues that one request (headers from the WebView,
 * redirects within the origin followed here), holds the response open, and
 * replaces it for the page with an empty {@code 204}. The bytes are never
 * requested twice.
 *
 * <p>Cookies stay the WebView's own. The request carries exactly the {@code
 * Cookie} header the browser would send, from the bundled AndroidX WebKit's
 * {@code COOKIE_INTERCEPT} feature (SameSite and partitioned cookies
 * included); neither {@code CookieManager.getCookie} nor a hand-built jar
 * stands in for it.
 *
 * <p>A redirect is followed only where its cookie context is provably the
 * same: the same origin, no {@code Set-Cookie} on the redirect response,
 * and the same path. Even an empty original Cookie header cannot prove that
 * the next path has no cookies. Anything else is a visible error rather than
 * a request with invented or forwarded credentials.
 *
 * <p>Cookies a response sets go back to the WebView through {@code
 * WebResourceResponseCompat.setCookies}, so the browser applies its own
 * SameSite, partition and expiry rules. The cookies of an intermediate
 * redirect cannot be handed over that way, which is why such a redirect is
 * refused. A WebView without the feature does not intercept and says so.
 *
 * <p>The request asks for no content encoding; if a {@code gzip} response
 * arrives anyway it is decoded before it is served or held (other encodings
 * are a visible error), so compressed bytes are never written as a file.
 * An intercepted download claims no declared size: the end reports exactly
 * what was written (the core checks a size only when one is claimed).
 *
 * <p>A redirect that leaves the request's origin is not followed: the new
 * origin's credentials belong to the WebView's cookie jar, which a native
 * request cannot read faithfully, and forwarding this origin's would leak
 * them. The page sees an error for that request; nothing is replayed.
 *
 * <p>No download state machine lives here: {@link #held} maps an id to its
 * live native object until the core answers with a command, and the core
 * owns everything after that.
 */
final class WebDownloads {
    private static final String TAG = "nsh";

    /** The length of a copy's buffer: the transfer is streamed, never held whole. */
    private static final int CHUNK = 64 * 1024;
    /** How often a copy reports its progress. */
    private static final long PROGRESS_BYTES = 1024 * 1024;
    private static final long PROGRESS_MILLIS = 500;
    /** The most redirects one held request follows. */
    private static final int MAX_REDIRECTS = 10;

    /** A download the view holds: what the core's offer is made of. */
    static final class Offer {
        final String id, url, filename, mimeType, method, source;
        final long bytes;
        final boolean canDelegate;

        Offer(String id, String url, String filename, String mimeType, long bytes, String method,
                String source, boolean canDelegate) {
            this.id = id;
            this.url = url;
            this.filename = filename;
            this.mimeType = mimeType;
            this.bytes = bytes;
            this.method = method;
            this.source = source;
            this.canDelegate = canDelegate;
        }
    }

    /** What the WebBox sends to the core (one JSON object per event). */
    interface Sink {
        void offer(Offer offer);

        void progress(String id, long received);

        /** The copy finished: exactly {@code bytes} went into the staging file. */
        void finished(String id, long bytes);

        void failed(String id, String code, String message);

        /** Answers a {@code Command::WebViewRequest}; {@code error} null is {@code ok:true}. */
        void reply(int request, String error);
    }

    /**
     * A held download's life, so the caller can keep its report lease until
     * every one of them (and every late report) is done.
     */
    interface Lifecycle {
        void retained();

        void retired();
    }

    private final Context context;
    private final Sink sink;
    private final Lifecycle lifecycle;
    private final ConcurrentHashMap<String, Held> held = new ConcurrentHashMap<>();
    /**
     * Admission of new held downloads: closed when the view is disposed, so
     * a response still arriving on an interception thread cannot add a Held
     * after the teardown pass has looked at the map. A Held admitted before
     * the close is seen by that pass.
     */
    private final Object admission = new Object();
    private boolean closed;

    /** One held response: the native object, and the writer while one runs. */
    private static final class Held {
        final Offer offer;
        final HttpURLConnection connection;
        final InputStream stream;

        // Under `this`.
        boolean started, done, cancelled, teardown, retired;
        int startRequest = -1;
        int cancelRequest = -1;
        boolean startAnswered, cancelAnswered;

        Held(Offer offer, HttpURLConnection connection, InputStream stream) {
            this.offer = offer;
            this.connection = connection;
            this.stream = stream;
        }
    }

    WebDownloads(Context context, Sink sink, Lifecycle lifecycle) {
        this.context = context;
        this.sink = sink;
        this.lifecycle = lifecycle;
    }

    /**
     * A new download id: unique across this process and every other, since
     * the core names the staging directory after it.
     */
    private static String nextId() {
        return "a" + java.util.UUID.randomUUID().toString();
    }

    // --- Scope ---------------------------------------------------------------

    /**
     * The app's {@code download_scope} patterns as matchers: a pattern is the
     * whole URL, where {@code *} stands for any run of characters (so
     * {@code https://example.com/files/*} covers that directory and
     * {@code https://example.com/export} covers exactly that URL).
     */
    static Pattern[] compile(String[] patterns) {
        Pattern[] compiled = new Pattern[patterns.length];
        for (int i = 0; i < patterns.length; i++) {
            StringBuilder regex = new StringBuilder("^");
            int start = 0;
            for (int at = patterns[i].indexOf('*'); at >= 0; at = patterns[i].indexOf('*', start)) {
                regex.append(Pattern.quote(patterns[i].substring(start, at))).append(".*");
                start = at + 1;
            }
            regex.append(Pattern.quote(patterns[i].substring(start))).append('$');
            compiled[i] = Pattern.compile(regex.toString(), Pattern.DOTALL);
        }
        return compiled;
    }

    private static boolean inScope(Pattern[] scope, String url) {
        for (Pattern pattern : scope) {
            if (pattern.matcher(url).matches()) return true;
        }
        return false;
    }

    /**
     * Enables the WebView's own request cookies in {@code
     * shouldInterceptRequest}: with them, the intercepted headers are the
     * ones the page's request would carry, SameSite and partitioned cookies
     * included. The library is bundled with the runtime, so only the
     * device's WebView has to support the feature.
     */
    static boolean enableRequestCookies(WebSettings settings) {
        if (!WebViewFeature.isFeatureSupported(WebViewFeature.COOKIE_INTERCEPT)) {
            Log.w(TAG, "web view: this WebView does not support COOKIE_INTERCEPT");
            return false;
        }
        try {
            WebSettingsCompat.setCookiesIncludedInShouldInterceptRequest(settings, true);
            return WebSettingsCompat.areCookiesIncludedInShouldInterceptRequest(settings);
        } catch (RuntimeException error) {
            Log.w(TAG, "web view: request cookies could not be enabled: " + error);
            return false;
        }
    }

    // --- Requests ------------------------------------------------------------

    /**
     * The response for {@code request} when it is an in-scope GET: the held
     * attachment (with the offer sent) or the response the page should see.
     * Null asks the WebView to carry the request as usual.
     */
    WebResourceResponse intercept(Pattern[] scope, WebResourceRequest request) {
        String url = request.getUrl().toString();
        if (!"GET".equalsIgnoreCase(request.getMethod())) return null;
        if (!"http".equalsIgnoreCase(request.getUrl().getScheme())
                && !"https".equalsIgnoreCase(request.getUrl().getScheme())) {
            return null;
        }
        if (!inScope(scope, url)) return null;

        Map<String, String> headers = new HashMap<>();
        for (Map.Entry<String, String> header : request.getRequestHeaders().entrySet()) {
            headers.put(header.getKey(), header.getValue());
        }
        try {
            return exchange(url, headers);
        } catch (Exception error) {
            // The single request may already have reached the server: asking
            // the WebView to retry would repeat its effects. An error
            // response is truthful and visible to the page.
            Log.w(TAG, "web view: a scoped request failed: " + error);
            return failure();
        }
    }

    /** The response head and, when held, the stream the core copies later. */
    private WebResourceResponse exchange(String url, Map<String, String> headers)
            throws IOException {
        // Hop-by-hop fields belong to this connection, not to the page. The
        // WebView's exact Cookie header for this request is kept apart: a
        // hop that keeps the origin and path keeps that same context, and
        // no other hop is followed.
        String cookie = header(headers, "Cookie");
        for (String name : new String[] {"Host", "Connection", "Keep-Alive", "Proxy-Connection",
                "Transfer-Encoding", "Content-Length", "Accept-Encoding", "Upgrade", "Expect", "TE",
                "Trailer", "Cookie"}) {
            headers.remove(caseInsensitiveKey(headers, name));
        }
        String current = url;
        // A scoped request blocks a WebView network thread until its head is
        // here: neither redirects nor a slow server may hold it for long.
        long deadline = System.nanoTime() + 60_000_000_000L;
        for (int hop = 0; ; hop++) {
            if (System.nanoTime() > deadline) {
                throw new IOException("no response head within 60 s");
            }
            HttpURLConnection connection = open(current, headers, cookie);
            try {
                int status = connection.getResponseCode();
                Map<String, String> response = responseHeaders(connection);
                if (isRedirect(status)) {
                    String location = header(response, "Location");
                    String next = location == null ? null
                            : new URL(new URL(current), location).toString();
                    List<String> cookiesSet = responseCookies(connection);
                    connection.disconnect();
                    if (next == null || hop >= MAX_REDIRECTS) {
                        throw new IOException("a redirect without a usable Location, or too many");
                    }
                    if (!sameOrigin(current, next)) {
                        // The new origin's credentials are the WebView's
                        // cookie jar's; this request cannot reproduce them,
                        // and forwarding this origin's would leak them.
                        throw new IOException("a redirect leaves " + origin(current) + " for "
                                + origin(next));
                    }
                    if (!cookiesSet.isEmpty()) {
                        // WebView would send what the hop set on the next
                        // request; only a response it processes can carry
                        // cookies, and a redirect response it never sees.
                        throw new IOException("a redirect set cookies, whose next request"
                                + " context this transport cannot reproduce");
                    }
                    if (!path(current).equals(path(next))) {
                        // The next path can have cookies absent from this request,
                        // even when this request's Cookie header was empty.
                        throw new IOException("a redirect changes the request cookie path from "
                                + path(current) + " to " + path(next));
                    }
                    current = next;
                    continue;
                }
                return respond(connection, current, url, status, response);
            } catch (IOException | RuntimeException error) {
                connection.disconnect();
                throw error;
            }
        }
    }

    private WebResourceResponse respond(HttpURLConnection connection, String url, String requested,
            int status, Map<String, String> headers) throws IOException {
        String encoding = header(headers, "Content-Encoding");
        boolean gzip = encoding != null
                && ("gzip".equalsIgnoreCase(encoding) || "x-gzip".equalsIgnoreCase(encoding));
        if (encoding != null && !"identity".equalsIgnoreCase(encoding) && !gzip) {
            // Never write or serve bytes whose encoding this cannot undo as
            // if they were the file.
            throw new IOException("the response is " + encoding + "-encoded, which this transport"
                    + " cannot decode");
        }
        Map<String, String> pageHeaders = new LinkedHashMap<>(headers);
        if (gzip) {
            pageHeaders.remove(caseInsensitiveKey(pageHeaders, "Content-Encoding"));
            pageHeaders.remove(caseInsensitiveKey(pageHeaders, "Content-Length"));
        }
        List<String> cookies = responseCookies(connection);
        String mime = header(headers, "Content-Type");
        String disposition = header(headers, "Content-Disposition");
        boolean attachment = status == 200 && isAttachment(disposition);
        InputStream body;
        if (status == 204 || status == 205 || status == 304) {
            body = new ByteArrayInputStream(new byte[0]);
        } else {
            body = status >= 400 ? connection.getErrorStream() : connection.getInputStream();
            if (body == null) body = new ByteArrayInputStream(new byte[0]);
        }
        if (gzip) {
            // Served and held decoded, as the browser would render it.
            body = new GZIPInputStream(body);
        }
        if (!attachment && !url.equals(requested)) {
            // A redirect led here: the WebView cannot be told the response's
            // real URL (its intercepted responses carry no 3xx and no final
            // URL), and serving this body as the requested URL would
            // misrepresent it. The page gets a visible error instead.
            throw new IOException("a scoped response redirected from " + requested + " to " + url);
        }
        if (attachment) {
            String id = nextId();
            String filename = filename(disposition, url);
            String type = mimeType(mime);
            // No declared size is claimed: a decoded body's length is not
            // the header's, and the end reports the bytes actually written.
            Offer offer = new Offer(id, url, filename, type, -1, "GET", "request", true);
            Held download = new Held(offer, connection, body);
            synchronized (admission) {
                if (closed) {
                    // Tearing down: no new Held may appear after the pass
                    // that stops and reports them.
                    close(download);
                    return failure();
                }
                held.put(id, download);
                lifecycle.retained();
            }
            sink.offer(offer);
            WebResourceResponseCompat response = new WebResourceResponseCompat("text/plain",
                    "utf-8", 204, "No Content", new HashMap<>(),
                    new ByteArrayInputStream(new byte[0]));
            if (!cookies.isEmpty()) response.setCookies(cookies);
            return response.toWebResourceResponse();
        }
        String type = mimeType(mime);
        String reason = connection.getResponseMessage();
        if (reason == null || reason.trim().isEmpty()) reason = "HTTP " + status;
        WebResourceResponseCompat response = new WebResourceResponseCompat(
                type == null ? "application/octet-stream" : type, charset(mime), status, reason,
                responseHeadersForPage(pageHeaders), closing(body, connection));
        if (!cookies.isEmpty()) response.setCookies(cookies);
        return response.toWebResourceResponse();
    }

    /**
     * A request the engine discarded: the listener's {@code None} offer. Its
     * declared length is the engine's, the only size this side still knows;
     * the method is unknown (a URL proves none) and so is that this request
     * could be reproduced.
     */
    void listenerDownload(String url, String contentDisposition, String mime, long contentLength) {
        synchronized (admission) {
            if (closed) return;
        }
        String id = nextId();
        String filename = filename(contentDisposition, url);
        Offer offer = new Offer(id, url, filename, mimeType(mime),
                contentLength >= 0 ? contentLength : -1, null, "none", false);
        sink.offer(offer);
    }

    /** A {@code WebViewOperation} Android has no mechanism for: answer, never leave it waiting. */
    void unsupported(int request) {
        sink.reply(request, "ERR_FEATURE_UNAVAILABLE_ON_PLATFORM: unsupported web view operation");
    }

    /** A scoped request whose response could not be reproduced: an error, never a retry. */
    private static WebResourceResponse failure() {
        return new WebResourceResponse("text/plain", "utf-8", 502, "Bad Gateway",
                new HashMap<>(), new ByteArrayInputStream(new byte[0]));
    }

    private static HttpURLConnection open(String url, Map<String, String> headers, String cookie)
            throws IOException {
        HttpURLConnection connection = (HttpURLConnection) new URL(url).openConnection();
        // Followed here: a replayed redirect through the WebView would be a
        // second request with effects of its own.
        connection.setInstanceFollowRedirects(false);
        connection.setRequestMethod("GET");
        connection.setUseCaches(false);
        // No content encoding: the bytes this response describes are the
        // bytes the file will receive (and no decoding can shorten them
        // behind the length's back).
        connection.setRequestProperty("Accept-Encoding", "identity");
        connection.setConnectTimeout(30_000);
        // Headers only: the copy raises it for the body.
        connection.setReadTimeout(30_000);
        for (Map.Entry<String, String> header : headers.entrySet()) {
            connection.setRequestProperty(header.getKey(), header.getValue());
        }
        // The WebView's exact header for this request, before any redirect:
        // the same origin keeps the browser's request context.
        if (cookie != null) connection.setRequestProperty("Cookie", cookie);
        return connection;
    }

    // --- Commands ------------------------------------------------------------

    /** {@code StartDownload}: copies the held bytes into {@code staging}. */
    void start(String id, String staging, int request) {
        Held download = held.get(id);
        if (download == null) {
            sink.reply(request, "ERR_NSH_DOWNLOAD_STATE: no held download " + id);
            return;
        }
        Thread writer;
        synchronized (download) {
            if (download.started || download.cancelled) {
                sink.reply(request, "ERR_NSH_DOWNLOAD_STATE: download " + id + " is not pending");
                return;
            }
            download.started = true;
            download.startRequest = request;
            writer = new Thread(() -> copy(download, staging), "nsh-download " + id);
            writer.setDaemon(true);
        }
        writer.start();
    }

    /**
     * {@code CancelDownload}: stops the writer (and the held request when it
     * never started), and answers once it has stopped — from the writer's
     * own thread when one runs, so nothing blocks the UI.
     */
    void cancel(String id, int request) {
        Held download = held.get(id);
        if (download == null) {
            // Nothing of this side's is writing (a `None` offer, or a
            // transfer that already stopped): there is nothing left to
            // stop, and the core may clean.
            sink.reply(request, null);
            return;
        }
        boolean answer = false;
        boolean pending = false;
        synchronized (download) {
            if (download.done) {
                // The transfer already stopped; the core will drop this.
                answer = true;
            } else if (!download.started) {
                download.cancelled = true;
                download.done = true;
                close(download);
                answer = true;
                pending = true;
            } else if (download.cancelRequest < 0) {
                download.cancelled = true;
                download.cancelRequest = request;
                close(download);
            } else {
                sink.reply(request, "ERR_NSH_DOWNLOAD_STATE: download " + id + " is already cancelling");
                return;
            }
        }
        if (answer) {
            sink.reply(request, null);
        }
        if (pending) {
            retire(download);
        }
    }

    /**
     * {@code DelegateDownload}: a held, known GET is handed to the system
     * as a new request in the system downloader's network context. The app
     * explicitly chooses this instead of accepting the original response.
     */
    void delegate(String id, int request) {
        Held download = held.get(id);
        if (download == null) {
            sink.reply(request, "ERR_NSH_DOWNLOAD_DISCARDED: no held download " + id);
            return;
        }
        if (!download.offer.canDelegate) {
            sink.reply(request, "ERR_NSH_DOWNLOAD_DISCARDED: the request's method is unknown");
            return;
        }
        synchronized (download) {
            // Delegation is a decision for a pending offer: a writer that
            // already runs (or ran) owns the bytes.
            if (download.started || download.done) {
                sink.reply(request, "ERR_NSH_DOWNLOAD_STATE: the download already writes");
                return;
            }
        }
        try {
            DownloadManager.Request managed = new DownloadManager.Request(Uri.parse(download.offer.url));
            managed.setTitle(download.offer.filename);
            if (download.offer.mimeType != null) managed.setMimeType(download.offer.mimeType);
            managed.setNotificationVisibility(DownloadManager.Request.VISIBILITY_VISIBLE_NOTIFY_COMPLETED);
            managed.setDestinationInExternalPublicDir(Environment.DIRECTORY_DOWNLOADS,
                    download.offer.filename);
            DownloadManager manager =
                    (DownloadManager) context.getSystemService(Context.DOWNLOAD_SERVICE);
            if (manager == null) throw new IllegalStateException("no DownloadManager");
            manager.enqueue(managed);
            synchronized (download) {
                download.done = true;
            }
            close(download);
            sink.reply(request, null);
            retire(download);
        } catch (Throwable error) {
            Log.w(TAG, "web view: the download could not be delegated: " + error);
            sink.reply(request, "ERR_NSH_DOWNLOAD_DISCARDED: " + error);
        }
    }

    /** The view is going away: stop every writer without waiting for the core. */
    void dispose() {
        synchronized (admission) {
            closed = true;
        }
        for (Held download : held.values().toArray(new Held[0])) {
            boolean alone;
            synchronized (download) {
                if (download.done) continue;
                download.teardown = true;
                download.cancelled = true;
                close(download);
                alone = !download.started;
                if (alone) download.done = true;
            }
            if (alone) {
                sink.failed(download.offer.id, "ERR_NSH_DOWNLOAD_TEARDOWN",
                        "the web view was disposed");
                retire(download);
            }
        }
    }

    // --- The writer ----------------------------------------------------------

    private void copy(Held download, String staging) {
        FileOutputStream out = null;
        boolean opened = false;
        long received = 0;
        try {
            if (download.cancelled) {
                stopped(download);
                return;
            }
            // The copy may legitimately take long between bytes; the
            // header wait above is what must stay short.
            download.connection.setReadTimeout(120_000);
            out = new FileOutputStream(new File(staging));
            opened = true;
            // The worker started: the core may publish only once the end
            // arrives, so this answer means "writing", not "written".
            synchronized (download) {
                download.startAnswered = true;
            }
            sink.reply(download.startRequest, null);
            long reported = 0;
            long reportAt = System.nanoTime();
            byte[] buffer = new byte[CHUNK];
            while (true) {
                if (download.cancelled) {
                    closeQuietly(out);
                    out = null;
                    stopped(download);
                    return;
                }
                int read = download.stream.read(buffer);
                if (read < 0) break;
                if (read == 0) continue;
                out.write(buffer, 0, read);
                received += read;
                long now = System.nanoTime();
                if (!download.cancelled
                        && (received - reported >= PROGRESS_BYTES
                                || (now - reportAt) / 1_000_000 >= PROGRESS_MILLIS)) {
                    reported = received;
                    reportAt = now;
                    sink.progress(download.offer.id, received);
                }
            }
            out.flush();
            closeQuietly(out);
            out = null;
            boolean cancelled;
            synchronized (download) {
                cancelled = download.cancelled || download.teardown;
                download.done = true;
            }
            close(download);
            if (cancelled) {
                stopped(download);
            } else {
                sink.finished(download.offer.id, received);
            }
        } catch (Exception error) {
            closeQuietly(out);
            out = null;
            boolean quiet;
            synchronized (download) {
                quiet = download.cancelled || download.teardown || download.done;
                download.done = true;
            }
            close(download);
            if (quiet) {
                stopped(download);
            } else if (!opened) {
                // Nothing was written: the core fails the start itself.
                synchronized (download) {
                    download.startAnswered = true;
                }
                sink.reply(download.startRequest,
                        "ERR_NSH_DOWNLOAD_DESTINATION: " + error);
            } else {
                Log.w(TAG, "web view: a download stopped: " + error);
                sink.failed(download.offer.id, "ERR_NSH_DOWNLOAD_NETWORK", String.valueOf(error));
            }
        } finally {
            closeQuietly(out);
            retire(download);
        }
    }

    /**
     * One held download is over: it leaves the map and its life is told,
     * exactly once (the writer, or the synchronous path that had no writer).
     */
    private void retire(Held download) {
        synchronized (download) {
            if (download.retired) return;
            download.retired = true;
        }
        held.remove(download.offer.id);
        lifecycle.retired();
    }

    /**
     * The writer (or the held request) stopped: answer the cancel if one
     * waits, else report a teardown. Exactly one of them, once.
     */
    private void stopped(Held download) {
        int cancel, start;
        boolean teardown;
        synchronized (download) {
            cancel = download.cancelRequest >= 0 && !download.cancelAnswered
                    ? download.cancelRequest : -1;
            if (cancel >= 0) download.cancelAnswered = true;
            // A start that never answered (cancelled before its worker
            // began) is answered too: the core waits for both commands.
            start = !download.startAnswered && download.startRequest >= 0
                    ? download.startRequest : -1;
            if (start >= 0) download.startAnswered = true;
            teardown = download.teardown && cancel < 0;
            download.done = true;
        }
        if (start >= 0) {
            sink.reply(start, "ERR_NSH_DOWNLOAD_STATE: cancelled before writing");
        }
        if (cancel >= 0) {
            sink.reply(cancel, null);
        } else if (teardown) {
            sink.failed(download.offer.id, "ERR_NSH_DOWNLOAD_TEARDOWN", "the web view was disposed");
        }
    }

    private static void close(Held download) {
        try {
            download.stream.close();
        } catch (Throwable ignored) {
            // Already closed, or the connection went away.
        }
        download.connection.disconnect();
    }

    /** Gives the page the body, and the connection back when it is done. */
    private static InputStream closing(InputStream body, HttpURLConnection connection) {
        return new java.io.FilterInputStream(body) {
            @Override
            public void close() throws IOException {
                try {
                    super.close();
                } finally {
                    connection.disconnect();
                }
            }
        };
    }

    private static void closeQuietly(OutputStream out) {
        if (out == null) return;
        try {
            out.close();
        } catch (Throwable ignored) {
            // The writer is stopping anyway.
        }
    }

    // --- Response helpers ----------------------------------------------------

    private static boolean isRedirect(int status) {
        return status == 301 || status == 302 || status == 303 || status == 307 || status == 308;
    }

    private static String origin(String url) {
        try {
            URL parsed = new URL(url);
            int port = parsed.getPort();
            if (port < 0) port = parsed.getDefaultPort();
            return parsed.getProtocol().toLowerCase(Locale.ROOT) + "://"
                    + parsed.getHost().toLowerCase(Locale.ROOT) + ":" + port;
        } catch (Throwable error) {
            return url;
        }
    }

    private static boolean sameOrigin(String a, String b) {
        return origin(a).equalsIgnoreCase(origin(b));
    }

    /** The URL's path, which is what cookie scoping follows. */
    private static String path(String url) {
        try {
            String path = new URL(url).getPath();
            return path == null || path.isEmpty() ? "/" : path;
        } catch (Throwable error) {
            return url;
        }
    }

    private static Map<String, String> responseHeaders(HttpURLConnection connection) {
        Map<String, String> headers = new LinkedHashMap<>();
        for (Map.Entry<String, List<String>> field : connection.getHeaderFields().entrySet()) {
            if (field.getKey() == null || field.getValue().isEmpty()) continue; // the status line
            if (!headers.containsKey(field.getKey())) headers.put(field.getKey(), field.getValue().get(0));
        }
        return headers;
    }

    /**
     * What the page sees of the response: the end-to-end fields, without the
     * ones this connection consumed (framing, encoding, cookies).
     */
    private static Map<String, String> responseHeadersForPage(Map<String, String> headers) {
        Map<String, String> page = new LinkedHashMap<>();
        for (Map.Entry<String, String> header : headers.entrySet()) {
            String name = header.getKey().toLowerCase(Locale.ROOT);
            if (name.equals("content-encoding") || name.equals("transfer-encoding")
                    || name.equals("connection") || name.equals("keep-alive")
                    || name.equals("set-cookie") || name.equals("trailer")) {
                continue;
            }
            page.put(header.getKey(), header.getValue());
        }
        return page;
    }

    private static List<String> responseCookies(HttpURLConnection connection) {
        List<String> cookies = new ArrayList<>();
        for (Map.Entry<String, List<String>> field : connection.getHeaderFields().entrySet()) {
            if (field.getKey() != null && "Set-Cookie".equalsIgnoreCase(field.getKey())) {
                cookies.addAll(field.getValue());
            }
        }
        return cookies;
    }

    private static String header(Map<String, String> headers, String name) {
        for (Map.Entry<String, String> header : headers.entrySet()) {
            if (header.getKey().equalsIgnoreCase(name)) return header.getValue();
        }
        return null;
    }

    private static String caseInsensitiveKey(Map<String, String> headers, String name) {
        for (String key : headers.keySet()) {
            if (key.equalsIgnoreCase(name)) return key;
        }
        return name;
    }

    /** The MIME type alone, or null when the response named none. */
    private static String mimeType(String contentType) {
        if (contentType == null) return null;
        int semicolon = contentType.indexOf(';');
        String mime = (semicolon < 0 ? contentType : contentType.substring(0, semicolon)).trim();
        return mime.isEmpty() ? null : mime;
    }

    private static String charset(String contentType) {
        if (contentType == null) return null;
        for (String parameter : contentType.split(";")) {
            int equals = parameter.indexOf('=');
            if (equals < 0) continue;
            if (!parameter.substring(0, equals).trim().equalsIgnoreCase("charset")) continue;
            String charset = parameter.substring(equals + 1).trim();
            if (charset.startsWith("\"") && charset.endsWith("\"") && charset.length() > 1) {
                charset = charset.substring(1, charset.length() - 1);
            }
            return charset.isEmpty() ? null : charset;
        }
        return null;
    }

    /** Whether {@code Content-Disposition} says the response is a file. */
    private static boolean isAttachment(String disposition) {
        if (disposition == null) return false;
        int semicolon = disposition.indexOf(';');
        String type = (semicolon < 0 ? disposition : disposition.substring(0, semicolon)).trim();
        return type.equalsIgnoreCase("attachment");
    }

    /**
     * The file name {@code Content-Disposition} suggests, else the last
     * segment of the URL's path; never a path.
     */
    static String filename(String disposition, String url) {
        if (disposition != null) {
            String extended = dispositionParameter(disposition, "filename*");
            if (extended != null) {
                String decoded = decodeExtended(extended);
                if (decoded != null && !decoded.isEmpty()) return sanitize(decoded);
            }
            String plain = dispositionParameter(disposition, "filename");
            if (plain != null && !plain.isEmpty()) return sanitize(plain);
        }
        try {
            String path = new URL(url).getPath();
            int slash = path.lastIndexOf('/');
            String name = percentDecode(slash < 0 ? path : path.substring(slash + 1), StandardCharsets.UTF_8);
            if (!name.isEmpty()) return sanitize(name);
        } catch (Throwable ignored) {
            // A URL this cannot parse: no name to suggest.
        }
        return "download";
    }

    /** One {@code Content-Disposition} parameter, its quotes removed. */
    private static String dispositionParameter(String disposition, String name) {
        for (String part : disposition.split(";")) {
            int equals = part.indexOf('=');
            if (equals < 0) continue;
            if (!part.substring(0, equals).trim().equalsIgnoreCase(name)) continue;
            String value = part.substring(equals + 1).trim();
            if (value.length() >= 2 && value.startsWith("\"") && value.endsWith("\"")) {
                value = value.substring(1, value.length() - 1);
            }
            return value;
        }
        return null;
    }

    /** {@code charset'language'percent-encoded} as the text it names. */
    private static String decodeExtended(String value) {
        int first = value.indexOf('\'');
        int second = first < 0 ? -1 : value.indexOf('\'', first + 1);
        if (second < 0) return null;
        Charset charset;
        try {
            charset = Charset.forName(value.substring(0, first));
        } catch (Throwable error) {
            charset = StandardCharsets.UTF_8;
        }
        return percentDecode(value.substring(second + 1), charset);
    }

    private static String percentDecode(String value, Charset charset) {
        java.io.ByteArrayOutputStream bytes = new java.io.ByteArrayOutputStream();
        for (int i = 0; i < value.length(); i++) {
            char c = value.charAt(i);
            if (c == '%' && i + 2 < value.length()) {
                try {
                    bytes.write(Integer.parseInt(value.substring(i + 1, i + 3), 16));
                    i += 2;
                    continue;
                } catch (NumberFormatException ignored) {
                    // A literal percent sign.
                }
            }
            byte[] encoded = String.valueOf(c).getBytes(charset);
            bytes.write(encoded, 0, encoded.length);
        }
        return new String(bytes.toByteArray(), charset);
    }

    /** A name with no directory part and no control characters. */
    private static String sanitize(String name) {
        StringBuilder clean = new StringBuilder(name.length());
        for (int i = 0; i < name.length(); i++) {
            char c = name.charAt(i);
            if (c == '/' || c == '\\') clean.append('_');
            else if (c >= 0x20 && c != 0x7f) clean.append(c);
        }
        String result = clean.toString().trim();
        while (result.startsWith(".")) result = result.substring(1);
        if (result.isEmpty()) return "download";
        return result;
    }
}
