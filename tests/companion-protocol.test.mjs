import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import ts from "typescript";

const source = await readFile(
  new URL("../src/companion-protocol.ts", import.meta.url),
  "utf8",
);
const compiled = ts.transpileModule(source, {
  compilerOptions: {
    target: ts.ScriptTarget.ES2022,
    module: ts.ModuleKind.ES2022,
  },
}).outputText;
const protocol = await import(
  `data:text/javascript;base64,${Buffer.from(compiled).toString("base64")}`
);

test("accepts a MediaStore photo page and preserves pagination", () => {
  assert.deepEqual(
    protocol.parsePhotoPage({
      photos: [{ id: "42", name: "camera.jpg", size: 1024, modified: 123 }],
      hasMore: true,
    }),
    {
      photos: [{ id: "42", name: "camera.jpg", size: 1024, modified: 123 }],
      hasMore: true,
    },
  );
});
test("rejects invalid photo identities and unbounded pages", () => {
  for (const id of ["../private", "", "-1"])
    assert.throws(() =>
      protocol.parsePhotoPage({
        photos: [{ id, name: "x", size: 1, modified: 1 }],
        hasMore: false,
      }),
    );
  assert.throws(() =>
    protocol.parsePhotoPage({
      photos: Array(61).fill({ id: "1", name: "x", size: 1, modified: 1 }),
      hasMore: false,
    }),
  );
});
test("rejects remote URLs in photo thumbnails", () => {
  assert.throws(() =>
    protocol.parseThumbnail({ dataUrl: "https://attacker.invalid/tracker" }),
  );
  assert.equal(
    protocol.parseThumbnail({ dataUrl: "data:image/jpeg;base64,/9j/" }),
    "data:image/jpeg;base64,/9j/",
  );
});
test("validates clipboard payload limits", () => {
  assert.equal(protocol.parseClipboard({ text: "hello" }), "hello");
  assert.throws(() =>
    protocol.parseClipboard({ text: "x".repeat(1024 * 1024 + 1) }),
  );
  assert.throws(() => protocol.parseClipboard({ text: null }));
});
