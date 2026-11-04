import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import ts from "typescript";

const source = await readFile(new URL("../src/clipboard-policy.ts", import.meta.url), "utf8");
const compiled = ts.transpileModule(source, {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ES2022 },
}).outputText;
const policy = await import(`data:text/javascript;base64,${Buffer.from(compiled).toString("base64")}`);
test("native clipboard preferences must be authoritative booleans", () => {
  assert.equal(policy.parseClipboardSyncPreference(false), false);
  assert.equal(policy.parseClipboardSyncPreference(true), true);
  for (const value of [null, undefined, "false", 0, {}])
    assert.throws(() => policy.parseClipboardSyncPreference(value));
});

test("temporary clipboard failures back off with a bounded delay and recover", () => {
  assert.deepEqual([0, 1, 2, 3, 20].map(policy.clipboardRetryDelay), [1500, 3000, 6000, 12000, 30000]);
  assert.equal(policy.clipboardRetryDelay(0), 1500);
});
test("empty and non-text selections do not erase the phone clipboard", () => {
  assert.equal(policy.canSendClipboard("", null, null), false);
  assert.equal(policy.canSendClipboard("", "phone text", "phone text"), false);
  assert.equal(policy.canSendClipboard("new text", "old text", "old text"), true);
});
test("incoming phone text supersedes an in-flight Linux clipboard snapshot", () => {
  assert.equal(policy.canSendClipboard("linux", "phone", "phone"), true);
  assert.equal(policy.canSendClipboard("linux", "old", "phone"), false);
  assert.equal(policy.canSendClipboard("phone", "phone", "phone"), false);
});
