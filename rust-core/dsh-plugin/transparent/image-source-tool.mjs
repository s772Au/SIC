import {isDeepStrictEqual} from 'node:util';
import {nativeToolResults} from './tool-linkage.mjs';

export const IMAGE_SOURCE_TOOL='sic_image_source';
export const IMAGE_SOURCE_RESULT='SIC archived image evidence. ';
export const IMAGE_SOURCE_DESCRIPTOR={
  name:IMAGE_SOURCE_TOOL,
  description:'Worker-only. Inspect one previous representation of the active I image lineage when visual detail is needed. Use the manifest_id shown in I. From description, retrieve the mosaic without a tile. From mosaic, supply a tile to retrieve its crop. From a delivered crop, supply its tile to retrieve the original image.',
  parameters:{type:'object',properties:{
    manifest_id:{type:'string',description:'Exact manifest_id shown in the active I view'},
    from:{type:'string',enum:['description','mosaic','crop']},
    tile:{type:'integer',minimum:1,description:'Numbered tile required from mosaic and crop'},
  },required:['manifest_id','from'],additionalProperties:false},
};
export function imageLookupResults(history){
  const pairs=nativeToolResults(history);
  return history.filter(message=>message.role==='tool'&&pairs.get(message.id)?.call.name===IMAGE_SOURCE_TOOL)
    .map(message=>({message,call:pairs.get(message.id).call}));
}

export function adoptedImageView(record){
  return record.layout_receipt?.image_view??(record.layout_receipt?null:record.host_binding?.context_projection?.image_view);
}

export function imagePredecessor(view,args) {
  if(!args || args.manifest_id!==view.pointer.manifest_id || !['description','mosaic','crop'].includes(args.from)
      || Object.keys(args).some(key=>!['manifest_id','from','tile'].includes(key))) throw Error('exact active image lineage is required');
  if(args.from==='description'){
    if(Object.hasOwn(args,'tile'))throw Error('description to mosaic accepts no tile');
    return {kind:'mosaic',item:view.lineage.mosaic};
  }
  if(!Number.isSafeInteger(args.tile)||args.tile<1)throw Error('numbered image lineage tile is required');
  const tile=view.lineage.mosaic.tiles.find(row=>row.number===args.tile);
  if(!tile)throw Error('image lineage tile was not delivered in this view');
  const crop=view.lineage.crops.find(row=>row.id===tile.crop_id);
  if(!crop)throw Error('image lineage crop is missing');
  if(args.from==='mosaic')return {kind:'crop',item:crop,tile:args.tile};
  const original=view.lineage.originals.find(row=>row.id===crop.parent_id);
  if(!original)throw Error('image lineage original is missing');
  return {kind:'original',item:original,tile:args.tile};
}

function priorNativeResult(view,history,kind,item,tile){
  const pairs=nativeToolResults(history);
  return history.some(message=>{
    if(message.role!=='tool'||message.isError===true)return false;
    const call=pairs.get(message.id)?.call;
    if(call?.name!==IMAGE_SOURCE_TOOL)return false;
    let args;
    try{args=JSON.parse(call.arguments);}catch{return false;}
    const expected=kind==='mosaic'
      ? {manifest_id:view.pointer.manifest_id,from:'description'}
      : {manifest_id:view.pointer.manifest_id,from:'mosaic',tile};
    return isDeepStrictEqual(args,expected)&&message.content.some(block=>
      block.type==='image'&&block.offloaded!==true&&isDeepStrictEqual(block.attachment,item.attachment));
  });
}

/** Grant a read from the exact admitted native model response and step. */
export function createImageSourceTool({imageCompression,bindingFor,clientFor,liveAgent,stepOf,store}){
  const pending=new WeakMap();
  function begin(agent){pending.delete(agent);}
  function finish(agent,projection,history,boundary,calls){
    const view=adoptedImageView(projection);
    if(!view?.active||!calls.size)return;
    pending.set(agent,{boundary,projectionId:projection.id,history,calls,view});
  }
  async function execute(args,exec){
    const {agent,callId,signal}=exec;
    const grant=pending.get(agent);
    const current=bindingFor(agent?.session.id,agent);
    if(!grant||!current||current.role==='EDITOR'||liveAgent(agent.id)!==agent
        || !isDeepStrictEqual(stepOf(agent.session),grant.boundary)
        || !isDeepStrictEqual(grant.calls.get(callId),args))throw Error('current admitted Worker image lookup call required');
    const record=await store.get(agent.session.id,grant.projectionId);
    if(!record||!isDeepStrictEqual(adoptedImageView(record),grant.view))throw Error('durable image lookup projection mismatch');
    const core=clientFor(current),source=await core.get({source_id:record.source_id},signal);
    if(source.source_digest!==record.source_digest)throw Error('authorized image lookup source differs');
    const currentCut=await core.operate('cut.get',{},signal);
    const expectedCut=record.layout_receipt?.selected_cut_id??record.host_binding?.context_projection?.selected_cut_id;
    if(currentCut.cut_id!==expectedCut)throw Error('image lookup view was superseded');
    const view=grant.view;
    const choice=imagePredecessor(view,args);
    if(args.from==='mosaic'&&view.pointer.display==='description'
        && !priorNativeResult(view,grant.history,'mosaic',view.lineage.mosaic))throw Error('inspect the mosaic before retrieving its crop');
    if(args.from==='crop'){
      const row=view.lineage.mosaic.tiles.find(tile=>tile.number===args.tile);
      const crop=view.lineage.crops.find(item=>item.id===row.crop_id);
      if(view.pointer.display!=='crops'&&!priorNativeResult(view,grant.history,'crop',crop,args.tile))throw Error('inspect the selected crop before retrieving its original');
    }
    const attachment=await imageCompression.assetReference(agent.session.id,view.pointer,choice.item.id,signal);
    const verifiedCut=await core.operate('cut.get',{},signal);
    if(verifiedCut.cut_id!==expectedCut)throw Error('image lookup view was superseded while reading');
    if(!attachment||pending.get(agent)!==grant||!isDeepStrictEqual(stepOf(agent.session),grant.boundary)
        || liveAgent(agent.id)!==agent||bindingFor(agent.session.id,agent)?.chain_id!==current.chain_id)throw Error('image lookup authority changed');
    return {manifest_id:args.manifest_id,lineage_digest:view.pointer.lineage_digest,kind:choice.kind,asset_id:choice.item.id,
      ...choice.tile?{tile:choice.tile}:{},attachment};
  }
  function render(_args,value){
    const {attachment,...lineage}=value;
    return [{type:'text',text:IMAGE_SOURCE_RESULT+JSON.stringify(lineage)},{type:'image',attachment}];
  }
  return {begin,finish,execute,render};
}

export function imageCallsFromMessage(message){
  const calls=new Map();
  for(const part of message.content){
    if(part.type!=='tool-call'||part.name!==IMAGE_SOURCE_TOOL||typeof part.id!=='string')continue;
    if(calls.has(part.id))throw Error('ambiguous native image lookup call identity');
    try{calls.set(part.id,JSON.parse(part.arguments));}catch{/* native tool runtime rejects invalid arguments */}
  }
  return calls;
}
