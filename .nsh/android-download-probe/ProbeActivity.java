package com.nsh.downloadprobe;

import android.app.Activity;
import android.app.DownloadManager;
import android.database.Cursor;
import android.net.Uri;
import android.os.Bundle;
import android.util.Base64;
import android.util.Log;
import android.webkit.JavascriptInterface;
import android.webkit.WebResourceRequest;
import android.webkit.WebResourceResponse;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import android.widget.TextView;
import java.io.*;
import java.net.*;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.util.*;
import java.util.concurrent.*;
import java.util.concurrent.atomic.AtomicInteger;
import org.json.JSONArray;
import org.json.JSONObject;

/** Standalone research probe; not NSH's download implementation. */
public final class ProbeActivity extends Activity {
    private WebView web;
    private String base;
    private final List<String> observations = Collections.synchronizedList(new ArrayList<>());
    private final BlockingQueue<String> downloads = new LinkedBlockingQueue<>();
    private final BlockingQueue<String> blobs = new LinkedBlockingQueue<>();
    private final AtomicInteger once = new AtomicInteger();
    private final AtomicInteger post = new AtomicInteger();
    private final AtomicInteger owned = new AtomicInteger();
    private final BlockingQueue<InputStream> ownedStreams = new LinkedBlockingQueue<>();
    private ServerSocket server;

    private void note(String message) {
        observations.add(message);
        Log.i("NSHProbe", message);
    }

    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        web = new WebView(this);
        setContentView(web);
        web.getSettings().setJavaScriptEnabled(true);
        web.addJavascriptInterface(new Object() {
            @JavascriptInterface public void bytes(String encoded) {
                byte[] bytes = Base64.decode(encoded, Base64.DEFAULT);
                String text = new String(bytes, StandardCharsets.UTF_8);
                blobs.offer("bytes=" + bytes.length + " exact=" + text.equals("probe-blob-content"));
            }
            @JavascriptInterface public void error(String error) { blobs.offer("error=" + error); }
        }, "Probe");
        web.setWebViewClient(new WebViewClient() {
            @Override public WebResourceResponse shouldInterceptRequest(WebView view, WebResourceRequest request) {
                note("INTERCEPT method=" + request.getMethod() + " url=" + request.getUrl()
                        + " main=" + request.isForMainFrame() + " headers=" + request.getRequestHeaders().keySet());
                if (request.getUrl().getPath().equals("/owned-once")) {
                    try {
                        final HttpURLConnection connection = (HttpURLConnection) new URL(request.getUrl().toString()).openConnection();
                        connection.setConnectTimeout(10000);
                        connection.setReadTimeout(10000);
                        for (Map.Entry<String, String> header : request.getRequestHeaders().entrySet()) connection.setRequestProperty(header.getKey(), header.getValue());
                        note("OWNED_RESPONSE status=" + connection.getResponseCode() + " disposition=" + connection.getHeaderField("Content-Disposition"));
                        ownedStreams.offer(new FilterInputStream(connection.getInputStream()) {
                            @Override public void close() throws IOException { try { super.close(); } finally { connection.disconnect(); } }
                        });
                        return new WebResourceResponse("text/plain", "UTF-8", 204, "No Content", Collections.emptyMap(), new ByteArrayInputStream(new byte[0]));
                    } catch (Exception error) {
                        note("PROBE_ERROR interception-first: " + error);
                        return new WebResourceResponse("text/plain", "UTF-8", 500, "Probe failure", Collections.emptyMap(), new ByteArrayInputStream(new byte[0]));
                    }
                }
                return null;
            }
        });
        web.setDownloadListener((url, ua, disposition, mime, length) -> {
            note("DOWNLOAD url=" + url + " disposition=" + disposition + " mime=" + mime + " length=" + length);
            downloads.offer(url);
        });
        note("RUNTIME android=" + android.os.Build.VERSION.SDK_INT + " webview=" + WebView.getCurrentWebViewPackage().versionName);
        new Thread(this::runProbe, "probe").start();
    }

    private void serve() {
        try {
            while (!server.isClosed()) {
                Socket socket = server.accept();
                new Thread(() -> {
                    try (Socket connection = socket) {
                        connection.setSoTimeout(10000);
                        BufferedReader input = new BufferedReader(new InputStreamReader(connection.getInputStream(), StandardCharsets.ISO_8859_1));
                        String first = input.readLine();
                        if (first == null) return;
                        String[] parts = first.split(" ");
                        String method = parts[0], path = parts[1];
                        int length = 0;
                        String line;
                        while ((line = input.readLine()) != null && !line.isEmpty()) {
                            if (line.toLowerCase(Locale.ROOT).startsWith("content-length:")) length = Integer.parseInt(line.substring(15).trim());
                        }
                        char[] requestBody = new char[length];
                        int offset = 0;
                        while (offset < length) {
                            int count = input.read(requestBody, offset, length - offset);
                            if (count < 0) break;
                            offset += count;
                        }
                        int status = 200;
                        String extra = "", type = "application/octet-stream", body = "original-response";
                        if (path.equals("/once")) {
                            int count = once.incrementAndGet();
                            if (count > 1) { status = 410; body = "already-consumed"; }
                        } else if (path.equals("/owned-once")) {
                            int count = owned.incrementAndGet();
                            if (count > 1) { status = 410; body = "already-consumed"; }
                        } else if (path.equals("/post")) {
                            post.incrementAndGet();
                            if (!method.equals("POST") || !new String(requestBody).equals("ticket=post-only")) {
                                status = 405; body = "POST-required";
                            }
                        } else if (path.equals("/redirect")) {
                            status = 302; extra = "Location: " + base + "/final\r\n";
                        } else if (path.equals("/blob")) {
                            type = "text/html";
                            body = "<!doctype html><title>blob probe</title><script>"
                                    + "onload=()=>{const b=new Blob(['probe-blob-content'],{type:'application/octet-stream'});"
                                    + "window.probeURL=URL.createObjectURL(b);let a=document.createElement('a');a.href=probeURL;a.download='probe.bin';document.body.append(a);a.click();};"
                                    + "</script><body>blob probe</body>";
                        } else if (!path.equals("/final")) {
                            type = "text/html"; body = "<!doctype html><body>probe</body>";
                        }
                        if (type.equals("application/octet-stream") && status == 200) extra += "Content-Disposition: attachment; filename=probe.bin\r\n";
                        note("SERVER method=" + method + " path=" + path + " bodyLength=" + offset + " status=" + status);
                        byte[] bytes = body.getBytes(StandardCharsets.UTF_8);
                        OutputStream output = connection.getOutputStream();
                        output.write(("HTTP/1.1 " + status + " Probe\r\nContent-Type: " + type + "\r\n" + extra
                                + "Content-Length: " + bytes.length + "\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n").getBytes(StandardCharsets.US_ASCII));
                        output.write(bytes);
                        output.flush();
                    } catch (Exception error) { note("SERVER_ERROR " + error.getClass().getSimpleName()); }
                }, "fixture-request").start();
            }
        } catch (IOException error) {
            if (!server.isClosed()) note("SERVER_ERROR " + error);
        }
    }

    private String navigate(String path, byte[] body) throws Exception {
        downloads.clear();
        runOnUiThread(() -> {
            if (body == null) web.loadUrl(base + path);
            else web.postUrl(base + path, body);
        });
        String url = downloads.poll(12, TimeUnit.SECONDS);
        note("OBSERVATION path=" + path + " callback=" + (url == null ? "none-within-12s" : url));
        return url;
    }

    private void replay(String url, String label) throws Exception {
        DownloadManager manager = (DownloadManager) getSystemService(DOWNLOAD_SERVICE);
        File destination = new File(getExternalFilesDir(null), "probe-" + label + ".bin");
        DownloadManager.Request request = new DownloadManager.Request(Uri.parse(url));
        request.setDestinationUri(Uri.fromFile(destination));
        request.setNotificationVisibility(DownloadManager.Request.VISIBILITY_VISIBLE);
        long id = manager.enqueue(request);
        try {
            long deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(20);
            while (System.nanoTime() < deadline) {
                try (Cursor cursor = manager.query(new DownloadManager.Query().setFilterById(id))) {
                    if (cursor != null && cursor.moveToFirst()) {
                        int status = cursor.getInt(cursor.getColumnIndexOrThrow(DownloadManager.COLUMN_STATUS));
                        int reason = cursor.getInt(cursor.getColumnIndexOrThrow(DownloadManager.COLUMN_REASON));
                        if (status == DownloadManager.STATUS_SUCCESSFUL || status == DownloadManager.STATUS_FAILED) {
                            note("REPLAY label=" + label + " status=" + status + " reason=" + reason);
                            return;
                        }
                    }
                }
                Thread.sleep(100);
            }
            note("REPLAY label=" + label + " timed-out");
        } finally { manager.remove(id); }
    }

    private void runProbe() {
        try {
            server = new ServerSocket(0, 8, InetAddress.getByName("127.0.0.1"));
            base = "http://127.0.0.1:" + server.getLocalPort();
            new Thread(this::serve, "fixture").start();
            String url = navigate("/once", null);
            if (url != null) replay(url, "one-use");
            note("ONE_USE_REQUEST_COUNT " + once.get());
            url = navigate("/post", "ticket=post-only".getBytes(StandardCharsets.US_ASCII));
            if (url != null) replay(url, "post");
            note("POST_ENDPOINT_REQUEST_COUNT " + post.get());
            navigate("/redirect", null);
            url = navigate("/blob", null);
            if (url != null && url.startsWith("blob:")) {
                try {
                    new DownloadManager.Request(Uri.parse(url));
                    note("BLOB_DOWNLOAD_MANAGER accepted-uri");
                } catch (IllegalArgumentException error) { note("BLOB_DOWNLOAD_MANAGER rejected-uri"); }
                // A separate cooperative-page experiment, not transparent native takeover.
                final String script = "fetch(" + JSONObject.quote(url) + ").then(r=>r.arrayBuffer()).then(b=>Probe.bytes(btoa(String.fromCharCode(...new Uint8Array(b))))).catch(e=>Probe.error(String(e)))";
                runOnUiThread(() -> web.evaluateJavascript(script, null));
                note("COOPERATIVE_BLOB " + blobs.poll(12, TimeUnit.SECONDS));
            }
            runOnUiThread(() -> web.loadUrl(base + "/owned-once"));
            InputStream response = ownedStreams.poll(12, TimeUnit.SECONDS);
            if (response == null) throw new IOException("no intercepted native response");
            File destination = new File(getFilesDir(), "owned-response.bin");
            note("OWNED_PENDING destinationExists=" + destination.exists());
            Thread.sleep(250);
            ByteArrayOutputStream bytes = new ByteArrayOutputStream();
            try (InputStream input = response; FileOutputStream output = new FileOutputStream(destination)) {
                byte[] buffer = new byte[1024];
                int count;
                while ((count = input.read(buffer)) != -1) { bytes.write(buffer, 0, count); output.write(buffer, 0, count); }
            }
            note("OWNED_ACCEPT requests=" + owned.get() + " bytes=" + bytes.size() + " exact=" + new String(bytes.toByteArray(), StandardCharsets.UTF_8).equals("original-response"));
        } catch (Exception error) { note("PROBE_ERROR " + error); }
        finally {
            try { if (server != null) server.close(); } catch (IOException ignored) {}
            try (FileOutputStream file = openFileOutput("observations.json", MODE_PRIVATE)) {
                synchronized (observations) { file.write(new JSONArray(observations).toString(2).getBytes(StandardCharsets.UTF_8)); }
            } catch (Exception error) { note("PROBE_ERROR saving observations: " + error); }
            note("PROBE_DONE");
            runOnUiThread(() -> {
                web.destroy();
                TextView text = new TextView(this);
                synchronized (observations) { text.setText(String.join("\n", observations)); }
                text.setTextSize(10);
                setContentView(text);
            });
        }
    }
}
