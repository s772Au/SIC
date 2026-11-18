import {test} from 'node:test';
import assert from 'node:assert/strict';
import {mkdtemp,mkdir,writeFile,readFile,readdir,rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {createFileProjectionStore} from '../transparent/file-store.mjs';
import {digest} from '../transparent/transport.mjs';

test('an interrupted legacy observation remains diagnosable without blocking the authoritative projection',async()=>{
  const root=await mkdtemp(join(tmpdir(),'sic-store-recovery-'));
  try{
    const store=createFileProjectionStore(root),adopted={messages:[]};
    const record={id:'p',session_id:'s',adopted,adopted_digest:digest(adopted)};
    await store.save(record);
    const events=join(root,digest('s'),'p.json.events');await mkdir(events);
    await writeFile(join(events,'1-interrupted.json'),'{"stage":"response-returned",');
    await writeFile(join(events,'2-unpublished.json.tmp'),'{"stage":');
    await store.observe('s','p',{stage:'response-returned',status:200});
    const actual=await createFileProjectionStore(root).get('s','p');
    assert.deepEqual(actual.adopted,adopted);
    assert.deepEqual(actual.outcomes,[{stage:'response-returned',status:200}]);
    assert.equal(actual.observation_errors.length,1);
    assert.equal(actual.observation_errors[0].file,'1-interrupted.json');
    assert.equal(await readFile(join(events,'1-interrupted.json'),'utf8'),'{"stage":"response-returned",');
    const entries=await readdir(events);
    assert.equal(entries.filter(n=>n.endsWith('.json')).length,2);
  }finally{await rm(root,{recursive:true,force:true});}
});

test('a torn trace tail is invisible to readers and repaired before the next durable append',async()=>{
  const root=await mkdtemp(join(tmpdir(),'sic-trace-recovery-'));
  try{
    const dir=join(root,digest('s'),'traces');await mkdir(dir,{recursive:true});
    const file=join(dir,digest('src')+'.jsonl');
    const first={stage:'editor-response',trace_sequence:1};
    await writeFile(file,JSON.stringify(first)+'\n{"stage":"editor-chunk","text":"half');
    const reopened=createFileProjectionStore(root);
    assert.deepEqual(await reopened.traces('s','src'),{rows:[first],next_offset:null});
    await reopened.recordTrace('s','src',{stage:'editor-response',text:'complete'});
    const final=await createFileProjectionStore(root).traces('s','src');
    assert.deepEqual(final.rows.map(r=>r.trace_sequence),[1,2]);
    assert.equal(final.rows[1].text,'complete');
    assert.equal((await readFile(file,'utf8')).includes('half'),false);
  }finally{await rm(root,{recursive:true,force:true});}
});

test('concurrent observations and trace appends retain complete records and paged sequence after reopen',async()=>{
  const root=await mkdtemp(join(tmpdir(),'sic-store-burst-'));
  try{
    const store=createFileProjectionStore(root),adopted={messages:[]};
    await store.save({id:'p',session_id:'s',adopted,adopted_digest:digest(adopted)});
    await Promise.all(Array.from({length:80},(_,n)=>store.observe('s','p',{stage:'response',n,text:'x'.repeat(8192)})));
    await Promise.all(Array.from({length:200},(_,n)=>store.recordTrace('s','src',{stage:'editor-chunk',n})));
    await store.recordTrace('s','src',{stage:'editor-response'});
    const reopened=createFileProjectionStore(root),record=await reopened.get('s','p');
    assert.equal(record.outcomes.length,80);
    assert.equal(new Set(record.outcomes.map(r=>r.n)).size,80);
    const page=await reopened.traces('s','src',195,10);
    assert.deepEqual(page.rows.map(r=>r.trace_sequence),[196,197,198,199,200,201]);
    assert.equal(page.next_offset,null);
    // Reopening another source must not require loading its full text into memory.
    await reopened.recordTrace('s','src',{stage:'editor-response'});
    assert.equal((await reopened.traces('s','src',201,10)).rows[0].trace_sequence,202);
  }finally{await rm(root,{recursive:true,force:true});}
});
