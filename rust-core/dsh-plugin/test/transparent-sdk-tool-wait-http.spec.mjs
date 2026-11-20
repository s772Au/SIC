import {test} from 'node:test';
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {mkdir, readFile, writeFile} from 'node:fs/promises';
import {join} from 'node:path';
import {pathToFileURL} from 'node:url';
import {setTimeout as delay} from 'node:timers/promises';
import {dshSourcePath, evidencePath, fixturePath} from './test-paths.mjs';
import {isolated} from './isolated-core-fixture.mjs';
import {apply} from '../transparent/plugin.mjs';
import {getEditorInvocation} from '../transparent/editor-invocation.mjs';
import {prepareSdkRuntime} from '../transparent/prepare-sdk-runtime.mjs';
import {prepareGoalRuntime} from '../transparent/prepare-goal-runtime.mjs';
import {prepareJobsRuntime} from '../transparent/prepare-jobs-runtime.mjs';
import {prepareDeepSeekFixture} from '../transparent/prepare-deepseek-fixture.mjs';

const dsh = dshSourcePath();
const load = path => import(pathToFileURL(join(dsh,path)).href);
const imported = path => import(pathToFileURL(path).href);
const build = fixturePath('sdk-tool-wait');
const evidence = evidencePath('sdk-tool-wait');
const [sdkPath, goal, jobs, messagesPath] = await Promise.all([
  prepareSdkRuntime(dsh,join(build,'sdk')),
  prepareGoalRuntime(dsh,join(build,'goal')),
  prepareJobsRuntime(dsh,join(build,'jobs')),
  prepareDeepSeekFixture(dsh,join(build,'messages')),
]);
const {Context} = await load('vendor/cordis/lib/index.js');
const sessionAPI = await load('packages/core/session/lib/index.js');
const api = await load('packages/llm/llm/lib/index.js');
const {agentEvents, default: AgentRegistry} = await load('packages/core/agent/lib/index.js');
const {renderPrompt, default: SystemPrompt} = await load('packages/core/system-prompt/lib/index.js');
const {DeepSeekAdapter, resolveAdapterOptions} = await imported(messagesPath);
const native = await readFile(new URL('../transparent/bundle/native.md',import.meta.url),'utf8');
const definitions = JSON.parse(await readFile(new URL('../transparent/bundle/native-tools.json',import.meta.url),'utf8'));
const progress = 'I established the observation boundary. I will now read the local file.';
const lateText = 'late exact file data 章';
const steering = 'Late steering arrived during native file IO.';
const incomingV = 'A previously published volatile note.';
const outgoingV = 'A volatile note for the next WB.';
const callId = 'f31-original-native-read';
const route = 'f31-native-messages';
const model = 'deepseek-v4-flash';
const textBlock = text => ({type:'text',text});
const user = text => api.createUserMessage({source:{kind:'user'},content:[textBlock(text)]});
const sha = value => createHash('sha256').update(JSON.stringify(value)).digest('hex');

function messagesSse(blocks) {
  const answer = (type,data={}) => ({type,...data});
  const events = [answer('message_start',{message:{id:'local-response',model,usage:{input_tokens:1,output_tokens:0}}})];
  blocks.forEach((block,index) => {
    const tool = block.type === 'tool_use';
    events.push(answer('content_block_start',{index,content_block:tool?{...block,input:{}}:textBlock('')}));
    events.push(answer('content_block_delta',{index,delta:tool
      ? {type:'input_json_delta',partial_json:JSON.stringify(block.input)} : {type:'text_delta',text:block.text}}));
    events.push(answer('content_block_stop',{index}));
  });
  events.push(answer('message_delta',{delta:{stop_reason:blocks.some(block=>block.type==='tool_use')?'tool_use':'end_turn'},usage:{output_tokens:1}}));
  events.push(answer('message_stop'));
  return new Response(events.map(event=>'event: '+event.type+'\ndata: '+JSON.stringify(event)+'\n\n').join(''),
    {headers:{'content-type':'text/event-stream'}});
}

function wireBlocks(payload) {
  return payload.messages.flatMap(message=>Array.isArray(message.content)?message.content:[textBlock(message.content)]);
}

function signalGate() {
  const gate = Promise.withResolvers();
  return {open:gate.resolve, wait:gate.promise};
}

async function runScenario(scenario) {
  const server = await isolated({evidenceRoot:join(build,'scratch')});
  const ctx = new Context(), started = performance.now(), timeline = [], worker = [], editors = [], errors = [];
  const ioStarted = signalGate(), allowIo = signalGate(), editorStarted = signalGate(), allowEditor = signalGate();
  const nativeResult = signalGate(), allowToolStart = signalGate(), publication = signalGate();
  const nextStep = signalGate();
  const mark = (stage, data={}) => timeline.push({stage,ms:performance.now()-started,...data});
  const at = stage => timeline.find(event=>event.stage===stage)?.ms;
  const payloadPath = join(server.root,'payload.txt');
  let agent, installed;
  const fails = scenario === 'editor-failure';
  const waitEditorFirst = scenario !== 'tool-first' && !fails;
  const beforeToolStart = scenario === 'editor-dispatched-before-tool-start';
  async function waitFor(label,promise) {
    const timeout = new AbortController();
    try {
      return await Promise.race([promise,delay(10000,undefined,{signal:timeout.signal}).then(()=>{
        const end=agent?.session.snapshotEvents().findLast(event=>event.type==='turn/end')?.data?.reason;
        throw Error('Timed out waiting for '+label+' '+JSON.stringify({timeline,errors,end}));
      })]);
    } finally { timeout.abort(); }
  }
  try {
    await writeFile(payloadPath,lateText+'\n');
    await ctx.plugin((await imported(sdkPath)).default);
    await ctx.plugin(sessionAPI.default);
    await ctx.plugin((await load('packages/session/session-projection/lib/index.js')).default);
    await ctx.plugin(SystemPrompt,{includeHarnessIdentity:false});
    await ctx.plugin((await load('packages/core/tools/lib/index.js')).default);
    await ctx.plugin(AgentRegistry);
    await ctx.plugin((await imported(goal.goal)).default);
    await ctx.plugin(await imported(goal.driver));
    await ctx.plugin((await imported(join(jobs.root,'jobs/jobs-local/index.mjs'))).default);
    await ctx.plugin(await imported(join(jobs.root,'jobs/tool-jobs/index.mjs')),{completionDelivery:'quiet'});
    await ctx.plugin(await load('packages/session/session-persistence-jsonl/lib/index.js'),{root:join(server.root,'sessions'),compression:'none'});
    await ctx.plugin((await load('packages/fs/fs-local/lib/index.js')).default,{cwd:server.root});
    await ctx.plugin((await load('packages/session/working-directory/lib/index.js')).default);
    await ctx.plugin(await load('packages/fs/tool-fs/lib/index.js'),{read:true,readImage:false,write:false,edit:false});
    ctx.provide('connection',{fetch:{register:()=>()=>{}}});
    ctx.systemPrompt.section({name:'reviewed-native',order:0,text:native,interpolate:false});
    ctx.on('system-prompt/assemble',async (_assembly,_context,next)=>{
      const value=await next();
      return {...value,sections:value.sections.filter(section=>{
        if (!['tool:jobs','tool:read'].includes(section.name)) return true;
        assert.ok(native.includes(section.text),'fixture uses the reviewed native tool guidance');
        return false;
      })};
    });
    for (const definition of definitions.filter(tool=>!['read','job_list','job_kill','job_output'].includes(tool.name))) {
      ctx.tools.register({...definition,output:{schema:{},render:()=>[]},execute:async()=>{throw Error('fixture dispatches only actual native read');}});
    }

    // Delay at the original filesystem service, inside the real ToolFs
    // executor. Its unchanged LocalFileSystem method reads the on-disk bytes.
    const read = ctx.fs.readText.bind(ctx.fs);
    ctx.fs.readText = async (target,signal) => {
      assert.equal(ctx.fs.processPath(target),payloadPath);
      mark('native-read-start');
      ioStarted.open();
      await allowIo.wait;
      const raw = await read(target,signal);
      assert.equal(raw,lateText+'\n');
      mark('native-read-end');
      return raw;
    };
    if (beforeToolStart) ctx.on('tools/pre-execute',async (exec,next)=>{
      if (exec.callId === callId) {
        mark('native-policy-before-tool-start');
        await allowToolStart.wait;
      }
      return next();
    });
    ctx.on('agent/error',payload=>errors.push(String(payload.error)));
    ctx.on('session/event',(session,event)=>{
      if (session !== agent?.session) return;
      if (event.type === 'assistant/message' && event.data.message.content.some(block=>block.id===callId)) {
        mark('worker-progress-committed',{message_id:event.data.message.id,event_seq:event.seq});
      }
      if (event.type === 'tool/result' && event.data.message.toolCallId===callId) {
        mark('native-result-committed',{message_id:event.data.message.id,event_seq:event.seq});
        nativeResult.open();
      }
      if (event.type === 'plugin:sic/tool-wait-settled') {
        mark('tool-wait-settled',event.data);
        publication.open();
      }
    });
    await ctx.plugin((await load('packages/core/agent-loop/lib/index.js')).default,{agents:[]});
    ctx.llm.registerAdapter([route],new DeepSeekAdapter({
      options:()=>resolveAdapterOptions({baseURL:'https://local-fixture.invalid/anthropic'}),
      resolveAuth:async()=>({headers:{'x-api-key':'local-fixture-only'}}),resolveUserId:()=> 'f31-fixture',
      resolveAttachments:()=>undefined,resolveImageAccess:()=>undefined,prepareExtensions:async()=>({fields:{},accept:async()=>{}}),
      fetch:async (_url,init,{options}) => {
        const wire = JSON.parse(init.body);
        const shared = options.messages.filter(message=>message.source?.kind==='sic-cb')
          .map(message=>({id:message.source.cb_id,text:message.content[0].text}));
        assert.equal(options.provider,route);
        assert.equal(wire.model,model);
        if (getEditorInvocation()) {
          const sourceId = getEditorInvocation().source_id;
          const source = await server.core.get({source_id:sourceId});
          const wb = JSON.parse(options.messages.at(-1).content[0].text);
          const snapshot = {source_id:sourceId,shared,units:wb.units,tool_wait:source.binding.tool_wait};
          editors.push(snapshot);
          if (snapshot.tool_wait) {
            mark('tool-wait-editor-start',{source_id:sourceId});
            editorStarted.open();
            await allowEditor.wait;
            assert.deepEqual(snapshot.tool_wait.boundary.call_ids,[callId]);
            assert.ok(wb.units.some(unit=>unit.text===progress));
            if (beforeToolStart) assert.ok(wb.units.some(unit=>unit.text===incomingV));
            assert.doesNotMatch(JSON.stringify(wire),new RegExp(lateText+'|'+steering));
            mark('tool-wait-editor-end');
            if (fails) return Response.json({error:{type:'api_error',message:'offline Editor failure'}},{status:500});
          }
          const ops = wb.units.map(unit=>({
            op:'COMPRESS',target:unit.unit_id,content:unit.text===progress?'F31 retained progress':unit.text,
          }));
          const pvi = beforeToolStart&&editors.length===1 ? {V:[incomingV]}
            : scenario==='editor-first'&&snapshot.tool_wait ? {V:[outgoingV]} : {};
          return messagesSse([{type:'tool_use',id:'delivery-'+editors.length,name:'sic_edit_blocks',input:{ops,pvi}}]);
        }
        worker.push({wire,shared});
        mark('worker-request',{index:worker.length});
        if (worker.length === 1) return messagesSse([textBlock(progress),
          {type:'tool_use',id:callId,name:'read',input:{file_path:'payload.txt'}}]);
        if (worker.length === 2) {
          const blocks=wireBlocks(wire), call=blocks.findIndex(block=>block.type==='tool_use'&&block.id===callId);
          const result=blocks.findIndex(block=>block.type==='tool_result'&&block.tool_use_id===callId);
          assert.ok(call>=0&&result>call,'native Messages result follows its original call');
          assert.equal(blocks[result].is_error,false);
          assert.match(JSON.stringify(blocks[result]),new RegExp(lateText));
          assert.match(JSON.stringify(wire),new RegExp(steering));
          if (scenario==='editor-first') assert.ok(wireBlocks(wire).some(block=>block.text?.includes(outgoingV)));
          return messagesSse([textBlock('Worker consumed the actual file contents.')]);
        }
        return messagesSse([textBlock('Worker completed the follow-up.')]);
      },
    }));
    const sessionId = 'f31-'+scenario, binding={chain_id:server.chain_id,scope_id:sessionId,role:'WORKER'};
    installed = await apply(ctx,{enabled:true,actor:'native-http-actor',coreURL:server.baseURL,credential:server.credential,
      projectionRoot:join(server.root,'projections'),bindingFor:id=>id===sessionId?binding:null,
      hierarchyPolicy:{max_level:2,level_batch_sizes:{2:8}},agentEvents,sessionAPI,llmAPI:api,renderPrompt});
    ctx.on('agent/pre-step',async (input,next)=>{
      if (input.agent===agent && input.turn===1 && input.step===2) nextStep.open();
      return next();
    },{prepend:true});
    agent = await ctx.agentLoop.create(sessionId,{provider:route,model,maxTokens:192},{cwd:server.root});
    agent.followup(user('Read the file and report the contents after observing it.'));
    if (beforeToolStart) {
      await waitFor('Editor before tool dispatch',editorStarted.wait);
      assert.ok(at('native-policy-before-tool-start')!==undefined);
      assert.equal(at('native-read-start'),undefined,'policy has not yet let the real tool start');
      allowToolStart.open();
    }
    await waitFor('real tool and Editor both started',Promise.all([editorStarted.wait,ioStarted.wait]));
    const oldCut = await server.core.operate('cut.get',{});
    assert.equal(oldCut.view.cbs.length,1);
    if (beforeToolStart) assert.equal((await server.core.operate('chain.get',{})).contexts.main.levels['1'].pending.length,1);
    const oldId = oldCut.view.cbs[0].cb_id, oldBlock=await server.core.operate('memory.replay',{cb_id:oldId});
    agent.steer(user(steering));
    if (waitEditorFirst) {
      allowEditor.open();
      await waitFor('tool-wait publication',publication.wait);
      assert.equal(at('native-read-end'),undefined);
      assert.equal(worker.length,1);
      assert.equal((await server.core.operate('cut.get',{})).view.cbs.length,2);
      allowIo.open();
    } else {
      allowIo.open();
      await waitFor('native result and next-step admission',Promise.all([nativeResult.wait,nextStep.wait]));
      await delay(75);
      assert.equal(worker.length,1,'no provider dispatch before the Editor completes');
      assert.equal((await server.core.operate('cut.get',{})).cut_id,oldCut.cut_id);
      allowEditor.open();
    }
    await waitFor('Worker continuation',agent.whenIdle());
    assert.deepEqual(errors,[]);
    assert.equal(worker.length,2);
    assert.equal(editors.length,2,'one normal initial Editor and one tool-wait Editor');
    const resultEvents = agent.session.snapshotEvents().filter(event=>event.type==='tool/result');
    assert.equal(resultEvents.length,1);
    const settled = timeline.find(event=>event.stage==='tool-wait-settled');
    assert.equal(settled.status,fails?'failed':'published');
    const boundary = editors[1].tool_wait.boundary;
    assert.equal(boundary.assistant_event_seq,timeline.find(event=>event.stage==='worker-progress-committed').event_seq);
    const expectedBody=beforeToolStart?incomingV+'\nF31 retained progress':'F31 retained progress';
    const expectedShared=fails?worker[0].shared:[...worker[0].shared,{id:settled.cb_id,text:expectedBody}];
    assert.deepEqual(worker[1].shared,expectedShared);
    assert.deepEqual(editors[1].shared,worker[0].shared);
    assert.deepEqual(worker[1].wire.system,worker[0].wire.system);
    assert.deepEqual(worker[1].wire.tools,worker[0].wire.tools);
    const openArchive=await server.core.get({source_id:editors[1].source_id});
    const fixed=fails?null:await server.core.operate('memory.replay',{cb_id:settled.cb_id});
    assert.doesNotMatch(JSON.stringify(openArchive.source_request),new RegExp(lateText+'|'+steering));
    if (fails) {
      assert.equal(openArchive.publication,null);
      assert.match(JSON.stringify(worker[1].wire),new RegExp(progress));
      const chain=await server.core.operate('chain.get',{});
      assert.deepEqual(chain.contexts.main.levels['1'].pending,[]);
    } else {
      assert.equal(openArchive.publication.publication.tool_wait.result_ownership,'native-host-next-boundary');
    }
    const joined=agent.session.snapshotEvents().find(event=>event.type==='plugin:sic/tool-wait-joined').data;
    assert.equal(joined.blocked,!waitEditorFirst);
    assert.ok(waitEditorFirst ? joined.wait_ms===0 : joined.wait_ms>=60);
    const overlap = Math.min(at('tool-wait-editor-end'),at('native-read-end'))-Math.max(at('tool-wait-editor-start'),at('native-read-start'));
    assert.ok(overlap>0);
    if (beforeToolStart) assert.ok(at('tool-wait-editor-start')<at('native-read-start'));
    assert.ok(at('worker-progress-committed')<at('tool-wait-editor-start'));
    assert.ok(timeline.find(event=>event.stage==='worker-request'&&event.index===2).ms>
      Math.max(at('native-result-committed'),at('tool-wait-settled')));

    // On the following boundary the already consumed result is eligible as
    // original tool material in a new WB, with its native message and call.
    agent.followup(user('Continue with the same source lineage.'));
    await waitFor('follow-up with late material',agent.whenIdle());
    assert.deepEqual(errors,[]);
    assert.equal(worker.length,3);
    assert.equal(editors.length,3);
    const late = editors[2].units.find(unit=>unit.text?.includes(lateText));
    assert.ok(late);
    assert.equal(late.host_origin.kind,'native-tool-result');
    assert.equal(late.host_origin.tool_call_id,callId);
    assert.equal(late.host_origin.message_id,resultEvents[0].data.message.id);
    assert.equal(late.host_origin.is_error,false);
    assert.ok(editors[2].units.some(unit=>unit.text===steering));
    if (scenario==='editor-first') assert.ok(editors[2].units.some(unit=>unit.text===outgoingV));
    const stable=[];
    for (const [id,before] of [[oldId,oldBlock],...(fixed?[[settled.cb_id,fixed]]:[])]) {
      const after=await server.core.operate('memory.replay',{cb_id:id});
      assert.deepEqual(after,before);
      stable.push({cb_id:id,before_sha256:sha(before),after_sha256:sha(after)});
    }
    const rows=(await installed.store.list(sessionId,0,20)).rows;
    assert.equal(rows.filter(row=>row.stage==='tool-wait-published').length,fails?0:1);
    assert.equal(rows.filter(row=>row.stage==='admitted-readonly').length,1);
    if (!fails) {
      const early=await installed.store.get(sessionId,rows.find(row=>row.stage==='tool-wait-published').id);
      assert.ok(!early.outcomes?.some(event=>event.stage.startsWith('adapter-stream')));
    }
    await mkdir(evidence,{recursive:true});
    await writeFile(join(evidence,scenario+'.json'),JSON.stringify({scenario,pass:true,provider_calls:0,
      adapter:'DSH DeepSeek Messages',tool:'DSH ToolFs read + original LocalFileSystem',overlap_ms:overlap,
      timeline,joined,stable_cbs:stable,native_tool_result:resultEvents[0],
      frozen_editor:editors[1],next_wb_tool_unit:late,worker_continuation:worker[1]},null,2)+'\n');
  } finally {
    for (const gate of [allowToolStart,allowIo,allowEditor]) gate.open();
    try { await ctx.fiber.dispose(); } finally { await server.close(); }
  }
}

for (const scenario of ['editor-first','tool-first','editor-dispatched-before-tool-start','editor-failure']) {
  test('actual DSH native read and first Editor overlap, '+scenario,{timeout:35000},()=>runScenario(scenario));
}
