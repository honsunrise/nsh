"""Compile and run a standalone Android WebView feasibility experiment."""
import json
import os
from pathlib import Path
import queue
import subprocess
import threading
import time
import zipfile

area = Path(__file__).resolve().parent
proof = area / 'proof'
proof.mkdir(parents=True, exist_ok=True)
sdk = Path(os.environ['ANDROID_HOME'])
tools = sdk / 'build-tools/35.0.0'
android = sdk / 'platforms/android-35/android.jar'
classes = area / 'classes'
dex = area / 'dex'
classes.mkdir(exist_ok=True)
dex.mkdir(exist_ok=True)

def run(args):
    result = subprocess.run([str(a) for a in args], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=180)
    print(result.stdout, flush=True)
    with (proof / 'build.log').open('a') as log:
        log.write(result.stdout)
    result.check_returncode()

manifest = area / 'AndroidManifest.xml'
manifest.write_text('''<manifest xmlns:android="http://schemas.android.com/apk/res/android" package="com.nsh.downloadprobe">
<uses-sdk android:minSdkVersion="26" android:targetSdkVersion="35" />
<uses-permission android:name="android.permission.INTERNET" />
<application android:debuggable="true" android:usesCleartextTraffic="true" android:label="NSH download probe" android:theme="@android:style/Theme.Material.Light.NoActionBar">
<activity android:name=".ProbeActivity" android:exported="true"><intent-filter><action android:name="android.intent.action.MAIN"/><category android:name="android.intent.category.LAUNCHER"/></intent-filter></activity>
</application></manifest>''')
run(['javac', '-source', '8', '-target', '8', '-classpath', android, '-d', classes, area / 'ProbeActivity.java'])
run(['jar', 'cf', area / 'classes.jar', '-C', classes, '.'])
run([tools / 'd8', '--lib', android, '--min-api', '26', '--output', dex, area / 'classes.jar'])
run([tools / 'aapt2', 'link', '-I', android, '--manifest', manifest, '-o', area / 'unsigned.apk'])
with zipfile.ZipFile(area / 'unsigned.apk', 'a') as apk:
    apk.write(dex / 'classes.dex', 'classes.dex')
run([tools / 'zipalign', '-f', '4', area / 'unsigned.apk', area / 'aligned.apk'])
run(['keytool', '-genkeypair', '-keystore', area / 'debug.keystore', '-storepass', 'android', '-keypass', 'android', '-alias', 'androiddebugkey', '-dname', 'CN=Android Debug', '-keyalg', 'RSA', '-keysize', '2048', '-validity', '30'])
run([tools / 'apksigner', 'sign', '--ks', area / 'debug.keystore', '--ks-pass', 'pass:android', '--out', area / 'probe.apk', area / 'aligned.apk'])
run(['adb', 'install', '-r', area / 'probe.apk'])
run(['adb', 'logcat', '-c'])
run(['adb', 'shell', 'am', 'start', '-n', 'com.nsh.downloadprobe/.ProbeActivity'])
logs = subprocess.Popen(['adb', 'logcat', '-v', 'brief', '-s', 'NSHProbe', 'AndroidRuntime'], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
messages = queue.Queue()
def collect():
    for line in logs.stdout:
        messages.put(line)
threading.Thread(target=collect, daemon=True).start()
complete = False
try:
    deadline = time.monotonic() + 150
    with (proof / 'native.log').open('w') as log:
        while time.monotonic() < deadline:
            try:
                line = messages.get(timeout=max(0.1, deadline - time.monotonic()))
            except queue.Empty:
                break
            log.write(line)
            log.flush()
            print(line, end='', flush=True)
            if 'PROBE_DONE' in line:
                complete = True
                break
            if 'FATAL EXCEPTION:' in line:
                break
    if not complete:
        raise RuntimeError('native probe did not complete')
    report = subprocess.check_output(['adb', 'exec-out', 'run-as', 'com.nsh.downloadprobe', 'cat', 'files/observations.json'], text=True)
    (proof / 'observations.json').write_text(report)
    with (proof / 'screenshot.png').open('wb') as image:
        subprocess.run(['adb', 'exec-out', 'screencap', '-p'], stdout=image, check=True)
    (proof / 'webview.txt').write_text(subprocess.check_output(['adb', 'shell', 'dumpsys', 'webviewupdate'], text=True))
    (proof / 'commit.txt').write_text(subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True))
    if any(line.startswith('PROBE_ERROR') for line in json.loads(report)):
        raise RuntimeError('probe encountered an unexpected execution error')
finally:
    logs.terminate()
    logs.wait(timeout=10)
    subprocess.run(['adb', 'shell', 'am', 'force-stop', 'com.nsh.downloadprobe'], check=False)
