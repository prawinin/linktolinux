import test from "node:test";
import assert from "node:assert/strict";
import {readFile} from "node:fs/promises";
import ts from "typescript";
const source=await readFile(new URL("../src/file-transfer-policy.ts",import.meta.url),"utf8");
const compiled=ts.transpileModule(source,{compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.ES2022}}).outputText;
const policy=await import(`data:text/javascript;base64,${Buffer.from(compiled).toString("base64")}`);
const transfer={id:"a1234567-1234-4567-890a-123456789012",deviceId:"phone",name:"Report.pdf",direction:"receive",size:4096,transferred:1024,status:"transferring",error:null,savedPath:null,startedAt:1234,finishedAt:null};
test("validates native transfer history, including provider-unknown and empty files",()=>{
 assert.deepEqual(policy.parseTransfers([transfer]),[transfer]);
 assert.equal(policy.parseTransfers([{...transfer,size:null}])[0].size,null);
 assert.equal(policy.parseTransfers([{...transfer,size:0,transferred:0,status:"complete",finishedAt:2000}])[0].size,0);
});
test("rejects malformed identities, names, counters, status and unbounded history",()=>{
 for(const patch of [{id:"../../escape"},{name:"../file"},{name:"bad\u0000file"},{transferred:-1},{size:8589934593},{status:"run"},{direction:"open"},{finishedAt:NaN}]) assert.throws(()=>policy.parseTransfers([{...transfer,...patch}]));
 assert.throws(()=>policy.parseTransfers(Array(101).fill(transfer)));
});
test("limits queue selections and reports progress without overflow",()=>{
 assert.deepEqual(policy.validateFileSelection(["/tmp/Report.pdf","/tmp/Report.pdf"]),["/tmp/Report.pdf"]);
 assert.throws(()=>policy.validateFileSelection(Array.from({length:17},(_,i)=>`/tmp/${i}`)));
 assert.equal(policy.transferProgress(transfer),25);
 assert.equal(policy.transferProgress({...transfer,size:null}),null);
 assert.equal(policy.transferProgress({...transfer,size:0,transferred:0,status:"complete"}),100);
 assert.equal(policy.transferProgress({...transfer,transferred:8192}),100);
 assert.equal(policy.formatTransferBytes(0),"0 B");
 assert.equal(policy.formatTransferBytes(1024),"1 KiB");
});

test("desktop operation deadlines release callers and preserve timely results", async()=>{
 assert.equal(await policy.withFileDeadline(Promise.resolve("ok"),50),"ok");
 await assert.rejects(policy.withFileDeadline(Promise.reject(new Error("bridge failed")),50),/bridge failed/);
 await assert.rejects(policy.withFileDeadline(new Promise(()=>{}),5),/timed out/);
});
test("poll retry delays remain bounded and recover immediately after success",()=>{
 assert.equal(policy.filePollDelay(0),1000);
 assert.equal(policy.filePollDelay(1),2000);
 assert.equal(policy.filePollDelay(20),15000);
});
