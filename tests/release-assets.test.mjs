import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import test from 'node:test';
const root = new URL('../', import.meta.url);
const read = (path) => readFile(new URL(path,root),'utf8');
test('Android launcher uses adaptive and themed branded icons',async()=>{
 const manifest = await read('android-companion/app/src/main/AndroidManifest.xml');
 assert.match(manifest,/android:icon="@mipmap\/ic_launcher"/);
 assert.match(manifest,/android:roundIcon="@mipmap\/ic_launcher_round"/);
 const adaptive = await read('android-companion/app/src/main/res/mipmap-anydpi-v26/ic_launcher.xml');
 assert.match(adaptive,/<monochrome/);
});
test('desktop launcher is portable and uses the application icon',async()=>{
 const config = JSON.parse(await read('src-tauri/tauri.conf.json'));
 assert.equal(config.app.enableGTKAppId,true);
 const launcher = await read('linktolinux.desktop');
 assert.match(launcher,/^Icon=dev\.linktolinux\.app$/m);
 assert.doesNotMatch(launcher,/\/home\//);
});
