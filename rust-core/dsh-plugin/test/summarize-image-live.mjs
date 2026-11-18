import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {readFile,writeFile} from 'node:fs/promises';
import {dirname,join} from 'node:path';
import {paidRowUpperCost} from './autodl-live-support.mjs';

const [summaryFile,reportFile]=process.argv.slice(2);
assert.ok(summaryFile&&reportFile,'supply one run summary and the output filename');
const directory=dirname(summaryFile),suffix='-summary.json';
assert.ok(summaryFile.endsWith(suffix),'expected the runner summary filename');
const summary=JSON.parse(await readFile(summaryFile,'utf8'));
const ledgerBytes=await readFile(join(directory,'paid-request-ledger.json'));
const ledger=JSON.parse(ledgerBytes);
const digest=bytes=>createHash('sha256').update(bytes).digest('hex');
const observations=new Map();
const cases=[];
for(const {key} of summary.completed){
  const caseFile=summaryFile.slice(0,-suffix.length)+'-'+key+'.json';
  const bytes=await readFile(caseFile),sample=JSON.parse(bytes);
  const rows=[];
  for(const request of sample.wire??[]){
    const arm=request.phase==='production_i'&&request.role==='editor'?'editor':request.phase;
    const usage=request.sdk_usage;
    const row={request_number:request.number,arm,role:request.role,http_status:request.http_status,
      usage:usage?{input_including_cache:usage.inputTokens+(usage.cacheReadTokens??0)+(usage.cacheWriteTokens??0),
        uncached:usage.inputTokens,cached_read:usage.cacheReadTokens??0,cached_write:usage.cacheWriteTokens??0,output:usage.outputTokens}:null,
      cost_upper_cny:request.cost_upper_cny,
      explicit_provider_visual_usage:request.explicit_visual_usage,
      dsh_estimated_visual_tokens:request.dsh_estimated_visual_tokens??null,
      wire_images:request.wire_images};
    rows.push(row);
    if(request.role==='worker'||arm==='editor')observations.set(arm,[...(observations.get(arm)??[]),row]);
  }
  cases.push({key,artifact:caseFile,sha256:digest(bytes),expected:sample.expected,coverage:sample.production?.coverage,
    answers:Object.fromEntries(Object.entries(sample.arms??{}).map(([name,answer])=>[name,{score:answer.score,error:answer.error}])),
    recall:sample.recall,rows,error:sample.error});
}
const aggregates={};
for(const [arm,rows] of observations){
  const measured=rows.filter(row=>row.usage!==null);
  const scores=cases.map(item=>item.answers[arm]?.score).filter(Boolean);
  const mean=field=>measured.length?measured.reduce((sum,row)=>sum+row.usage[field],0)/measured.length:null;
  aggregates[arm]={request_count:rows.length,usage_count:measured.length,task_count:scores.length,
    exact_cases:scores.filter(score=>score.exact).length,correct_items:scores.reduce((sum,score)=>sum+score.correct_items,0),
    expected_items:scores.reduce((sum,score)=>sum+score.expected_items,0),mean_input_including_cache:mean('input_including_cache'),
    mean_uncached:mean('uncached'),mean_cached_read:mean('cached_read'),mean_cached_write:mean('cached_write'),mean_output:mean('output'),
    mean_image_count:rows.reduce((sum,row)=>sum+row.wire_images.length,0)/rows.length,
    mean_png_bytes:rows.reduce((sum,row)=>sum+row.wire_images.reduce((size,image)=>size+image.bytes,0),0)/rows.length,
    mean_dsh_estimated_visual_tokens:rows.every(row=>row.dsh_estimated_visual_tokens!==null)?
      rows.reduce((sum,row)=>sum+row.dsh_estimated_visual_tokens,0)/rows.length:null,
    explicit_provider_visual_usage_count:rows.filter(row=>row.explicit_provider_visual_usage!==null).length,
    total_upper_cny:Number(rows.reduce((sum,row)=>sum+row.cost_upper_cny,0).toFixed(6))};
}
const report={summary:summaryFile,package:summary.packageRoot,core_sha256:summary.core_sha256,plugin_sha256:summary.plugin_sha256,
  model:summary.model,worker_settings:summary.settings,ledger:{sha256:digest(ledgerBytes),requests:ledger.requests.length,
    total_upper_cny:Number(ledger.requests.reduce((sum,row)=>sum+paidRowUpperCost(row),0).toFixed(6)),baseline_upper_cny:summary.baseline_upper_cny},
  measurement_notes:[
    'The four isolated Worker arms receive the same task, model, settings, system prompt and no tools. Isolated visual arms never receive answer-bearing Editor descriptions.',
    'Production I is a separate witness. Mosaic display includes Editor descriptions and full Harness context. The recall witness begins with description display.',
    'Provider input tokens are aggregate and include cache in the full total. Native DSH visual estimates use actual wire image dimensions and are not provider metering or billing.',
    'Cost is a conservative bound using the configured maximum rates, not an invoice.',
  ],
  aggregates,cases};
await writeFile(reportFile,JSON.stringify(report,null,2)+'\n');
