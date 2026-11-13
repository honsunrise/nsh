package com.longbridge.nsh;

import android.annotation.SuppressLint;
import android.app.Activity;
import android.content.Context;
import android.content.pm.PackageManager;
import android.graphics.Bitmap;
import android.os.Handler;
import android.os.Looper;
import android.view.MotionEvent;
import android.webkit.JavascriptInterface;
import android.webkit.PermissionRequest;
import android.webkit.WebChromeClient;
import android.webkit.RenderProcessGoneDetail;
import android.webkit.WebResourceError;
import android.webkit.WebResourceRequest;
import android.webkit.WebResourceResponse;
import android.webkit.WebSettings;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import java.util.ArrayList;
import java.util.List;
import java.nio.charset.StandardCharsets;
import java.util.HashMap;
import java.util.Objects;
import java.util.regex.Pattern;
import org.json.JSONObject;
import org.json.JSONTokener;

/**
 * `<webview>`: an {@code android.webkit.WebView}. Pages reach the app through
 * {@code window.nsh.postMessage};
 * the app's messages arrive as {@code message} events on {@code window}.
 *
 * <p>{@code incognito}: Android's WebView keeps cookies and DOM storage in
 * one store per app, which every web view shares and which stays. An
 * incognito web view only changes what is its own: it loads without the
 * HTTP cache and saves no form data.
 *
 * <p>When the page's renderer process ends (it crashed, or the system
 * reclaimed its memory), the app stays: this WebView is dropped, the app
 * hears {@code onError} with {@code ERR_NSH_WEBVIEW_TERMINATED}, and the
 * next {@code reload()}, {@code load()} or new source makes a new one.
 */
@SuppressLint({"ViewConstructor", "SetJavaScriptEnabled"})
final class WebBox extends WidgetBox {
    private WebView web;
    /** Source kind (0 none, 1 uri, 2 html) and fields; null before the first load. */
    private Object[] source;
    private String injected, injectedBefore, userAgent;
    private boolean scrollable = true;
    // The last props, to set up a WebView made after a renderer ended.
    private boolean javaScriptEnabled, incognito;
    private volatile boolean disposed, processGone;
    private final Handler evaluationHandler = new Handler(Looper.getMainLooper());
    private final HashMap<Integer, Evaluation> evaluations = new HashMap<>();
    /** Downloads: the held in-scope responses and their writer. */
    private final WebDownloads downloads;
    /** {@code download_scope}, compiled (empty: nothing is intercepted). */
    private volatile Pattern[] downloadScope = new Pattern[0];
    /** The core asked the view to hold downloads ({@code defer_downloads}). */
    private boolean deferDownloads;
    /** The node's event mask, to know whether a download is listened for. */
    private long eventsMask, eventsHigh;
    /** Whether downloads are held, readable from the interception thread. */
    private volatile boolean holdDownloads;
    /** Whether the WebView's own request cookies are available (tried once). */
    private boolean cookiesTried;
    private volatile boolean requestCookies;
    /** Whether the scope's missing feature was reported (once per web view). */
    private boolean scopeReported;
    /**
     * The download report lease: the engine control cloned when this view
     * can hold downloads, so stops reported during teardown still reach
     * that engine's cleanup sink after {@code Native.destroy}. It never
     * holds the host or this view, and is released once the view is
     * disposed and every held download ended.
     */
    private final Object downloadLeaseLock = new Object();
    private long downloadLease;
    private boolean downloadLeaseClosed, downloadsDisposed;
    private int heldDownloads;
    // The shared page wrapper enforces the result limit; leave room for its envelope.
    private static final int MAX_ENVELOPE_BYTES = 1048576 + 1024;

    private final class Evaluation {
        final int request;
        final String generation, pollScript, cleanupScript;
        final Runnable poll;

        Evaluation(int request, String generation, String pollScript, String cleanupScript) {
            this.request = request;
            this.generation = generation;
            this.pollScript = pollScript;
            this.cleanupScript = cleanupScript;
            poll = () -> runEvaluation(this, pollScript);
        }
    }

    private WebBox(Context context, Mounter mounter, int id, WebView web) {
        super(context, mounter, id, web);
        this.web = web;
        this.downloads = new WebDownloads(context, new Sink(), new DownloadsLifecycle());
        attach(web);
    }

    /** What the download transport reports, as the core's internal events. */
    private final class Sink implements WebDownloads.Sink {
        @Override
        public void offer(WebDownloads.Offer offer) {
            JSONObject json = new JSONObject();
            try {
                json.put("id", offer.id);
                json.put("url", offer.url);
                json.put("filename", offer.filename);
                json.put("mime_type", offer.mimeType == null ? JSONObject.NULL : offer.mimeType);
                json.put("bytes", offer.bytes < 0 ? JSONObject.NULL : Long.valueOf(offer.bytes));
                json.put("method", offer.method == null ? JSONObject.NULL : offer.method);
                json.put("source", offer.source);
                json.put("can_delegate", offer.canDelegate);
            } catch (org.json.JSONException error) {
                android.util.Log.w("nsh", "web view: a download offer could not be encoded: " + error);
                return;
            }
            event(Native.DOWNLOAD_OFFER, json);
        }

        @Override
        public void progress(String id, long received) {
            JSONObject json = new JSONObject();
            try {
                json.put("id", id);
                json.put("received", received);
            } catch (org.json.JSONException error) {
                return;
            }
            event(Native.DOWNLOAD_PROGRESS, json);
        }

        @Override
        public void finished(String id, long bytes) {
            JSONObject json = new JSONObject();
            try {
                json.put("id", id);
                json.put("outcome", "finished");
                json.put("bytes", bytes);
            } catch (org.json.JSONException error) {
                return;
            }
            event(Native.DOWNLOAD_END, json);
        }

        @Override
        public void failed(String id, String code, String message) {
            JSONObject json = new JSONObject();
            try {
                json.put("id", id);
                json.put("outcome", "failed");
                json.put("code", code == null ? JSONObject.NULL : code);
                json.put("message", message);
            } catch (org.json.JSONException error) {
                return;
            }
            event(Native.DOWNLOAD_END, json);
        }

        @Override
        public void reply(int request, String error) {
            JSONObject json = new JSONObject();
            try {
                json.put("id", request);
                json.put("ok", error == null);
                if (error != null) json.put("error", error);
            } catch (org.json.JSONException e) {
                return;
            }
            // A request's answer is internal: no listener selects it.
            event(Native.WEB_VIEW_REPLY, json);
        }

        private void event(int kind, JSONObject json) {
            // Through the lease, not the view: after `NshView.destroy` the
            // Java handle is gone, but the original engine's download
            // cleanup sink must still hear a late stop (and never another
            // app's engine).
            // Held across the call: release drops the lease under the same
            // lock, so a report can never outlive it. A UI-thread report may
            // acquire it late (the engine is installed while the first
            // batch is applied).
            synchronized (downloadLeaseLock) {
                if (downloadLease == 0 && !downloadLeaseClosed && !downloadsDisposed
                        && Looper.myLooper() == Looper.getMainLooper()) {
                    downloadLease = Native.downloadLease(mounter.handle());
                }
                if (downloadLease == 0) return;
                Native.downloadReport(downloadLease, id, kind, json.toString());
            }
        }
    }

    /** A held download's life, for the report lease. */
    private final class DownloadsLifecycle implements WebDownloads.Lifecycle {
        @Override
        public void retained() {
            synchronized (downloadLeaseLock) {
                heldDownloads++;
            }
        }

        @Override
        public void retired() {
            synchronized (downloadLeaseLock) {
                if (heldDownloads > 0) heldDownloads--;
            }
            releaseDownloadLease();
        }
    }

    /**
     * Clones this view's engine control for download reports, once: called
     * on the UI thread when the view starts holding downloads, before any
     * request. A null control (no engine yet) leaves it for the next call.
     */
    private void acquireDownloadLease() {
        synchronized (downloadLeaseLock) {
            if (downloadLease != 0 || downloadLeaseClosed || downloadsDisposed) return;
            downloadLease = Native.downloadLease(mounter.handle());
        }
    }

    /** Releases the lease once the view is disposed and nothing is held. */
    private void releaseDownloadLease() {
        synchronized (downloadLeaseLock) {
            if (!downloadsDisposed || heldDownloads != 0 || downloadLease == 0) return;
            Native.downloadLeaseRelease(downloadLease);
            downloadLease = 0;
            downloadLeaseClosed = true;
        }
    }

    private void attach(WebView web) {
        web.getSettings().setDomStorageEnabled(true);
        web.addJavascriptInterface(new Bridge(), "nsh");
        web.setWebViewClient(new Client());
        web.setWebChromeClient(new Chrome());
        web.setDownloadListener(this::downloadStarted);
        web.setOnTouchListener((v, e) -> {
            if (e.getActionMasked() == MotionEvent.ACTION_DOWN) {
                // The page scrolls itself: keep enclosing scroll views out.
                v.getParent().requestDisallowInterceptTouchEvent(true);
            }
            return !scrollable && e.getActionMasked() == MotionEvent.ACTION_MOVE;
        });
    }

    /**
     * A web view, or an empty box when the system has no WebView (no
     * provider installed or enabled: {@code new WebView} throws). The app
     * then hears {@code onError} with {@code ERR_FEATURE_UNAVAILABLE_ON_PLATFORM}
     * and shows what it has instead.
     */
    static Box make(Context context, Mounter mounter, int id) {
        WebView web;
        try {
            web = new WebView(context);
        } catch (RuntimeException e) {
            android.util.Log.w("nsh", "WebView is not available: " + e);
            mounter.eventMap(id, Native.ERROR, new String[] {"message", "code"},
                    new Object[] {"WebView is not available: " + e.getMessage(),
                            "ERR_FEATURE_UNAVAILABLE_ON_PLATFORM"});
            return new Box(context, mounter, id);
        }
        return new WebBox(context, mounter, id, web);
    }

    /** Called from the page on a binder thread. */
    private final class Bridge {
        @JavascriptInterface
        public void postMessage(String data) {
            post(() -> {
                if (!disposed && !processGone) {
                    mounter.eventMap(id, Native.MESSAGE, new String[] {"data"}, new Object[] {data});
                }
            });
        }
    }

    private final class Client extends WebViewClient {
        /** Top-level navigations stay within the app's `webview` capability. */
        @Override
        public boolean shouldOverrideUrlLoading(WebView view, WebResourceRequest request) {
            if (disposed || processGone) return true;
            if (!request.isForMainFrame()) return false;
            String url = request.getUrl().toString();
            if (mounter.allowsWebview(url)) return false;
            android.util.Log.w("nsh", "web view navigation to " + url + " is not granted");
            mounter.eventMap(id, Native.ERROR, new String[] {"message", "code", "url"},
                    new Object[] {"navigation to " + url + " is not granted", "ERR_ACCESS_DENIED", url});
            return true;
        }

        /**
         * An in-scope GET is NSH's own single request: the attachment is
         * held for the app and the page sees an empty response; anything
         * else is served from that same response. Null leaves the request
         * to the WebView.
         */
        @Override
        public WebResourceResponse shouldInterceptRequest(WebView view, WebResourceRequest request) {
            if (disposed || processGone || view != web) return null;
            // Without the WebView's own request cookies the request could not
            // be reproduced faithfully (§ COOKIE_INTERCEPT): the WebView
            // carries it instead, and its download is a `None` offer.
            Pattern[] scope = downloadScope;
            if (scope.length == 0 || !requestCookies || !holdsDownloads()) return null;
            return downloads.intercept(scope, request);
        }

        @Override
        public void onPageStarted(WebView view, String url, Bitmap favicon) {
            if (disposed || processGone) return;
            cancelEvaluations("ERR_NSH_WEBVIEW_ABORTED", "The document navigated");
            if (injectedBefore != null) view.evaluateJavascript(injectedBefore, null);
            state(Native.LOAD_START, url, true);
        }

        @Override
        public void onPageFinished(WebView view, String url) {
            if (disposed || processGone) return;
            if (injected != null) view.evaluateJavascript(injected, null);
            state(Native.LOAD, url, false);
        }

        @Override
        public void onReceivedError(WebView view, WebResourceRequest request, WebResourceError error) {
            if (disposed || processGone) return;
            if (!request.isForMainFrame()) return;
            mounter.eventMap(id, Native.ERROR, new String[] {"message", "url"},
                    new Object[] {String.valueOf(error.getDescription()), request.getUrl().toString()});
        }

        @Override
        public boolean onRenderProcessGone(WebView view, RenderProcessGoneDetail detail) {
            // Returning true for every WebView keeps the app alive: unhandled,
            // Android ends the app's process along with the renderer.
            if (disposed || processGone || view != web) return true;
            processGone = true;
            cancelEvaluations("ERR_NSH_WEBVIEW_ABORTED", "The web content process terminated");
            // A WebView with a lost renderer cannot be reused: drop it and
            // make a new one when the app asks for a page again.
            removeView(web);
            web.destroy();
            String message = detail.didCrash()
                    ? "The web content process crashed"
                    : "The system ended the web content process";
            android.util.Log.w("nsh", "web view: " + message);
            mounter.eventMap(id, Native.ERROR, new String[] {"message", "code"},
                    new Object[] {message, "ERR_NSH_WEBVIEW_TERMINATED"});
            return true;
        }
    }

    /** Media capture ({@code getUserMedia}): what the app's {@code webview.media} grants. */
    private final class Chrome extends WebChromeClient {
        @Override
        public void onPermissionRequest(PermissionRequest request) {
            List<String> granted = new ArrayList<>();
            List<String> system = new ArrayList<>();
            for (String resource : request.getResources()) {
                String kind, permission;
                if (PermissionRequest.RESOURCE_AUDIO_CAPTURE.equals(resource)) {
                    kind = "microphone";
                    permission = android.Manifest.permission.RECORD_AUDIO;
                } else if (PermissionRequest.RESOURCE_VIDEO_CAPTURE.equals(resource)) {
                    kind = "camera";
                    permission = android.Manifest.permission.CAMERA;
                } else {
                    continue;
                }
                if (!mounter.allowsWebMedia(kind)) {
                    android.util.Log.w("nsh", "web view " + kind + " capture is not granted: add it to capabilities.webview.media");
                    continue;
                }
                granted.add(resource);
                system.add(permission);
            }
            if (granted.isEmpty()) {
                request.deny();
                return;
            }
            String[] resources = granted.toArray(new String[0]);
            Context context = getContext();
            List<String> missing = new ArrayList<>();
            for (String p : system) {
                if (context.checkSelfPermission(p) != PackageManager.PERMISSION_GRANTED) missing.add(p);
            }
            if (missing.isEmpty()) {
                request.grant(resources);
            } else if (context instanceof Activity) {
                MediaPermissions.of((Activity) context).ask(missing.toArray(new String[0]), allowed -> {
                    if (allowed) request.grant(resources);
                    else request.deny();
                });
            } else {
                request.deny();
            }
        }
    }

    /** Asks the user for the system's media permissions, across configuration changes. */
    @SuppressWarnings("deprecation")
    public static final class MediaPermissions extends android.app.Fragment {
        private static final String TAG = "com.longbridge.nsh.WebBox.MediaPermissions";
        private static final int CODE = 0x4f01;
        private final List<java.util.function.Consumer<Boolean>> waiting = new ArrayList<>();

        static MediaPermissions of(Activity activity) {
            android.app.FragmentManager fragments = activity.getFragmentManager();
            MediaPermissions found = (MediaPermissions) fragments.findFragmentByTag(TAG);
            if (found == null) {
                found = new MediaPermissions();
                fragments.beginTransaction().add(found, TAG).commitNow();
            }
            return found;
        }

        @Override
        public void onCreate(android.os.Bundle state) {
            super.onCreate(state);
            setRetainInstance(true);
        }

        void ask(String[] permissions, java.util.function.Consumer<Boolean> answer) {
            waiting.add(answer);
            if (waiting.size() == 1) requestPermissions(permissions, CODE);
        }

        @Override
        public void onRequestPermissionsResult(int code, String[] permissions, int[] results) {
            if (code != CODE) {
                super.onRequestPermissionsResult(code, permissions, results);
                return;
            }
            boolean allowed = results.length > 0;
            for (int r : results) allowed &= r == PackageManager.PERMISSION_GRANTED;
            List<java.util.function.Consumer<Boolean>> answered = new ArrayList<>(waiting);
            waiting.clear();
            for (java.util.function.Consumer<Boolean> a : answered) a.accept(allowed);
        }
    }

    /**
     * Whether the core holds this view's downloads: it asked to defer them,
     * or something listens for the app's download events.
     */
    private boolean holdsDownloads() {
        return holdDownloads;
    }

    /** The node's new event mask (`Mounter`'s EVENTS), on the UI thread. */
    void events(long mask, long high) {
        eventsMask = mask;
        eventsHigh = high;
        recomputeHold();
    }

    private void recomputeHold() {
        holdDownloads = deferDownloads || wanted(Native.DOWNLOAD) || wanted(Native.DOWNLOAD_PENDING);
        if (holdDownloads) acquireDownloadLease();
    }

    private boolean wanted(int kind) {
        if (kind >= 64) return (eventsHigh & (1L << (kind - 64))) != 0;
        return (eventsMask & (1L << kind)) != 0;
    }

    /**
     * {@code DownloadListener}: the response is already gone (Chromium
     * discarded it), so only its metadata can be offered, and delegating it
     * would be a new request whose method this view never saw.
     */
    private void downloadStarted(String url, String userAgent, String contentDisposition,
            String mimeType, long contentLength) {
        if (disposed || processGone || !holdsDownloads()) return;
        downloads.listenerDownload(url, contentDisposition, mimeType, contentLength);
    }

    /**
     * Turns on the WebView's request cookies exactly once per WebView, and
     * says so when a declared scope cannot be honored.
     */
    private void ensureRequestCookies() {
        if (cookiesTried) return;
        cookiesTried = true;
        requestCookies = WebDownloads.enableRequestCookies(web.getSettings());
        if (!requestCookies && !scopeReported) {
            scopeReported = true;
            String message = "download_scope needs the WebView COOKIE_INTERCEPT feature,"
                    + " which this system WebView does not support: the scope issues no request";
            android.util.Log.w("nsh", "web view: " + message);
            mounter.eventMap(id, Native.ERROR, new String[] {"message", "code"},
                    new Object[] {message, "ERR_FEATURE_UNAVAILABLE_ON_PLATFORM"});
        }
    }

    private void state(int kind, String url, boolean loading) {
        String[] keys = {"url", "title", "loading", "canGoBack", "canGoForward"};
        Object[] values = {url, Objects.toString(web.getTitle(), ""), loading, web.canGoBack(), web.canGoForward()};
        if (mounter.wants(id, kind)) mounter.eventMap(id, kind, keys, values);
        if (mounter.wants(id, Native.NAVIGATION_STATE_CHANGE)) {
            mounter.eventMap(id, Native.NAVIGATION_STATE_CHANGE, keys, values);
        }
    }

    @SuppressWarnings("deprecation") // setSaveFormData: still honored before Android 8.
    void configure(Object[] source, String injected, String injectedBefore, String userAgent,
            boolean scrollable, boolean javaScriptEnabled, boolean incognito,
            String[] downloadScope, boolean deferDownloads) {
        if (disposed) return;
        this.deferDownloads = deferDownloads;
        recomputeHold();
        boolean revived = processGone;
        if (processGone && !revive()) return;
        this.downloadScope = WebDownloads.compile(downloadScope);
        if (downloadScope.length > 0) ensureRequestCookies();
        if (!javaScriptEnabled) cancelEvaluations("ERR_NSH_WEBVIEW_SCRIPT", "JavaScript is disabled");
        // Read with the first props: the core keeps it from changing.
        if (this.source == null && incognito) {
            WebSettings settings = web.getSettings();
            settings.setCacheMode(WebSettings.LOAD_NO_CACHE);
            settings.setSaveFormData(false);
        }
        boolean scriptsChanged = !Objects.equals(injected, this.injected)
                || !Objects.equals(injectedBefore, this.injectedBefore);
        this.injected = injected;
        this.injectedBefore = injectedBefore;
        this.scrollable = scrollable;
        this.javaScriptEnabled = javaScriptEnabled;
        if (this.source == null) this.incognito = incognito;
        web.setVerticalScrollBarEnabled(scrollable);
        web.setHorizontalScrollBarEnabled(scrollable);
        web.getSettings().setJavaScriptEnabled(javaScriptEnabled);
        if (!Objects.equals(userAgent, this.userAgent)) {
            this.userAgent = userAgent;
            web.getSettings().setUserAgentString(userAgent);
        }
        if (revived || this.source == null || !java.util.Arrays.equals(source, this.source) || scriptsChanged) {
            this.source = source;
            if ((int) source[0] != 0) {
                cancelEvaluations("ERR_NSH_WEBVIEW_ABORTED", "The document navigated");
            }
            loadSource();
        }
    }

    private void loadSource() {
        switch ((int) source[0]) {
            case 1: web.loadUrl((String) source[1]); break;
            case 2: web.loadDataWithBaseURL((String) source[2], (String) source[1], "text/html", "UTF-8", null); break;
            default: break;
        }
    }

    /**
     * A new WebView in place of the one whose renderer ended, set up with
     * the last props; false (the app hears why) when none can be made.
     */
    @SuppressWarnings("deprecation") // setSaveFormData, as in configure.
    private boolean revive() {
        WebView fresh;
        try {
            fresh = new WebView(getContext());
        } catch (RuntimeException e) {
            android.util.Log.w("nsh", "WebView is not available: " + e);
            mounter.eventMap(id, Native.ERROR, new String[] {"message", "code"},
                    new Object[] {"WebView is not available: " + e.getMessage(),
                            "ERR_FEATURE_UNAVAILABLE_ON_PLATFORM"});
            return false;
        }
        web = fresh;
        attach(fresh);
        replaceWidget(fresh);
        processGone = false;
        // The fresh WebView has its own settings: its request cookies must
        // be enabled again when a scope is set.
        cookiesTried = false;
        requestCookies = false;
        if (downloadScope.length > 0) ensureRequestCookies();
        WebSettings settings = fresh.getSettings();
        if (incognito) {
            settings.setCacheMode(WebSettings.LOAD_NO_CACHE);
            settings.setSaveFormData(false);
        }
        settings.setJavaScriptEnabled(javaScriptEnabled);
        if (userAgent != null) settings.setUserAgentString(userAgent);
        fresh.setVerticalScrollBarEnabled(scrollable);
        fresh.setHorizontalScrollBarEnabled(scrollable);
        return true;
    }

    /** Commands 4–9 of `Mounter.command`. */
    void command(int kind, String text) {
        if (disposed) return;
        if (processGone) {
            // reload() shows the app's page again in a new WebView.
            if (kind == 4 && source != null && revive()) loadSource();
            return;
        }
        if (kind >= 4 && kind <= 6) {
            cancelEvaluations("ERR_NSH_WEBVIEW_ABORTED", "The document navigated");
        }
        switch (kind) {
            case 4: web.reload(); break;
            case 5: web.goBack(); break;
            case 6: web.goForward(); break;
            case 7: web.stopLoading(); break;
            case 8: web.evaluateJavascript(text, null); break;
            case 9:
                web.evaluateJavascript("window.dispatchEvent(new MessageEvent('message', { data: "
                        + JSONObject.quote(text) + " }));", null);
                break;
            default: break;
        }
    }

    /**
     * Command 53 of `Mounter.command`: the core's {@code WebViewRequest}.
     * {@code operation} 0 starts the copy into {@code staging}, 1 cancels,
     * 2 delegates; anything else has no Android mechanism and is answered.
     */
    void download(int operation, int request, String id, String staging) {
        switch (operation) {
            case 0: downloads.start(id, staging, request); break;
            case 1: downloads.cancel(id, request); break;
            case 2: downloads.delegate(id, request); break;
            default: downloads.unsupported(request);
        }
    }

    /**
     * Command 46 of `Mounter.command` (`load()`): `url` with `headers` on its
     * own request; WebView sends them with no subresource or later navigation.
     */
    void load(String url, java.util.Map<String, String> headers) {
        if (disposed) return;
        if (processGone && !revive()) return;
        cancelEvaluations("ERR_NSH_WEBVIEW_ABORTED", "The document navigated");
        web.loadUrl(url, headers);
    }

    /** Starts a request without exposing a page-to-native bridge. */
    void evaluate(int request, String generation, String start, String poll, String cleanup) {
        if (disposed) return;
        if (processGone || !web.getSettings().getJavaScriptEnabled()) {
            evaluationError(request, generation, processGone ? "ERR_NSH_WEBVIEW_ABORTED" : "ERR_NSH_WEBVIEW_SCRIPT",
                    processGone ? "The web content process terminated" : "JavaScript is disabled");
            return;
        }
        cancelEvaluation(request);
        Evaluation evaluation = new Evaluation(request, generation, poll, cleanup);
        evaluations.put(request, evaluation);
        // The core keeps the deadline: it cancels the request when the
        // app's timeout passes.
        runEvaluation(evaluation, start);
    }

    private boolean active(Evaluation evaluation) {
        return !disposed && !processGone && evaluations.get(evaluation.request) == evaluation;
    }

    private void runEvaluation(Evaluation evaluation, String script) {
        if (!active(evaluation)) return;
        try {
            web.evaluateJavascript(script, encoded -> {
                if (!active(evaluation)) return;
                try {
                    // Android JSON-encodes the string returned by the shared
                    // wrapper. Decode that one extra layer, not the result value.
                    if (encoded == null) {
                        failEvaluation(evaluation, "ERR_NSH_WEBVIEW_SCRIPT", "WebView returned an invalid evaluation response");
                        return;
                    }
                    if (encoded.length() > MAX_ENVELOPE_BYTES * 6 + 2) {
                        failEvaluation(evaluation, "ERR_NSH_QUOTA_EXCEEDED", "The JavaScript result is too large");
                        return;
                    }
                    Object decoded = new JSONTokener(encoded).nextValue();
                    if (!(decoded instanceof String)) {
                        failEvaluation(evaluation, "ERR_NSH_WEBVIEW_SCRIPT", "WebView returned an invalid evaluation response");
                        return;
                    }
                    String envelope = (String) decoded;
                    if (envelope.getBytes(StandardCharsets.UTF_8).length > MAX_ENVELOPE_BYTES) {
                        failEvaluation(evaluation, "ERR_NSH_QUOTA_EXCEEDED", "The JavaScript result is too large");
                        return;
                    }
                    String status = new JSONObject(envelope).getString("status");
                    if (status.equals("pending")) {
                        evaluationHandler.postDelayed(evaluation.poll, 25);
                    } else if (status.equals("ok") || status.equals("error")) {
                        finishEvaluation(evaluation, envelope);
                    } else {
                        failEvaluation(evaluation, "ERR_NSH_WEBVIEW_SCRIPT", "WebView returned an invalid evaluation response");
                    }
                } catch (org.json.JSONException error) {
                    failEvaluation(evaluation, "ERR_NSH_WEBVIEW_SCRIPT", "WebView returned an invalid evaluation response");
                }
            });
        } catch (RuntimeException error) {
            failEvaluation(evaluation, "ERR_NSH_WEBVIEW_SCRIPT", "WebView could not evaluate JavaScript");
        }
    }

    private void evaluationReply(int request, String generation, String envelope) {
        // Internal completions do not depend on an app event listener.
        mounter.eventMap(id, Native.WEB_EVALUATION, new String[] {"id", "data", "generation"},
                new Object[] {Double.valueOf(Integer.toUnsignedLong(request)), envelope, generation});
    }

    private void evaluationError(int request, String generation, String code, String message) {
        evaluationReply(request, generation, "{\"status\":\"error\",\"code\":" + JSONObject.quote(code)
                + ",\"message\":" + JSONObject.quote(message) + "}");
    }

    private void finishEvaluation(Evaluation evaluation, String envelope) {
        if (!active(evaluation)) return;
        cancelEvaluation(evaluation.request);
        evaluationReply(evaluation.request, evaluation.generation, envelope);
    }

    private void failEvaluation(Evaluation evaluation, String code, String message) {
        if (!active(evaluation)) return;
        cancelEvaluation(evaluation.request);
        evaluationError(evaluation.request, evaluation.generation, code, message);
    }

    /** Core cancellation is silent and also removes the page's pending cell. */
    void cancelEvaluation(int request) {
        Evaluation evaluation = evaluations.remove(request);
        if (evaluation == null) return;
        evaluationHandler.removeCallbacks(evaluation.poll);
        if (!disposed && !processGone) {
            try {
                web.evaluateJavascript(evaluation.cleanupScript, null);
            } catch (RuntimeException ignored) {
                // A dead renderer has no remaining page cell to release.
            }
        }
    }

    private void cancelEvaluations(String code, String message) {
        for (Evaluation evaluation : evaluations.values().toArray(new Evaluation[0])) {
            cancelEvaluation(evaluation.request);
            evaluationError(evaluation.request, evaluation.generation, code, message);
        }
    }

    void destroy() {
        if (disposed) return;
        cancelEvaluations("ERR_NSH_WEBVIEW_ABORTED", "The web view was disposed");
        // Before the view is gone: stop every writer and report the
        // teardown, since the core's cancels may not arrive first. The
        // lease outlives the host, so those reports still reach this
        // engine's cleanup sink; a running writer retires it when it stops.
        synchronized (downloadLeaseLock) {
            downloadsDisposed = true;
        }
        downloads.dispose();
        releaseDownloadLease();
        disposed = true;
        evaluationHandler.removeCallbacksAndMessages(null);
        if (!processGone) {
            web.stopLoading();
            removeView(web);
            web.destroy();
        }
    }
}
