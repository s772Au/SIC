/* Optional DSH view for the actual adopted native request and its edit trace. */
window.__ModuleLoader__.load({id:'sic-transparent-host',factory:require=>{
  const React=require('react'),h=React.createElement;
  function ImageLineage({sessionId,projectionId,view}){
    const [preview,setPreview]=React.useState(null),[error,setError]=React.useState('');
    const pending=React.useRef(null);
    React.useEffect(()=>()=>pending.current?.abort(),[]);
    React.useEffect(()=>()=>{if(preview)URL.revokeObjectURL(preview.url);},[preview]);
    async function open(assetId,label){
      pending.current?.abort();const request=new AbortController();pending.current=request;
      setError('');
      try{
        const response=await fetch('api/sic-core/native-projections',{method:'POST',headers:{'content-type':'application/json'},
          body:JSON.stringify({sessionId,projectionId,action:'image',assetId}),signal:request.signal});
        if(!response.ok)throw Error('image unavailable');
        const blob=await response.blob();if(request.signal.aborted)return;
        setPreview({url:URL.createObjectURL(blob),label});
      }catch{if(!request.signal.aborted)setError('无法读取这条谱系里的图片。');}
    }
    const {mosaic,crops,originals,description}=view.lineage;
    return h('details',null,h('summary',null,'I 区图片与来源'),
      h('p',null,view.active?'该请求采用'+({mosaic:'拼接图',crops:'裁切图组',description:'带引用的文字描述'}[view.pointer.display])+'。':'该请求恢复了宿主原生图片投影，仍可回查这份归档。'),
      h('button',{onClick:()=>void open(mosaic.id,'I 区拼接图')},'查看拼接图'),
      ...description.parts.map((part,index)=>h('p',{key:index},part.text+'（图块 '+part.tiles.join('、')+'）')),
      ...mosaic.tiles.map(tile=>{
        const crop=crops.find(row=>row.id===tile.crop_id),original=originals.find(row=>row.id===crop.parent_id);
        return h('div',{key:tile.number},h('span',null,'图块 '+tile.number+' · 裁切坐标 '+JSON.stringify(crop.region)+' '),
          h('button',{onClick:()=>void open(crop.id,'裁切 '+tile.number)},'看裁切'),
          h('button',{onClick:()=>void open(original.id,'原图 '+tile.number)},'看原图'));
      }),error?h('p',{role:'alert'},error):null,
      preview?h('img',{src:preview.url,alt:preview.label,style:{display:'block',maxWidth:'100%',maxHeight:560,objectFit:'contain'}}):null);
  }
  function NativeTrace({sessionId}){
    const[rows,setRows]=React.useState([]),[record,setRecord]=React.useState(null),[error,setError]=React.useState(''),[history,setHistory]=React.useState(null);
    const[nextListOffset,setNextListOffset]=React.useState(null),epoch=React.useRef(0),selection=React.useRef(null);
    React.useEffect(()=>{epoch.current++;selection.current=null;setRows([]);setRecord(null);setHistory(null);setNextListOffset(null);setError('');},[sessionId]);
    async function query(action,projectionId,offset=0,part='all'){
      const cursors=action==='history'&&part!=='all'?{
        after_version:part==='versions'?history.core.next_after_version:(history.core.versions?.at(-1)?.version??0),
        after_i_revision:part==='i'?history.core.next_after_i_revision:(history.core.i_projections?.at(-1)?.revision??0),
      }:{};
      if(action==='get'){epoch.current++;setRecord(null);setHistory(null);}
      const generation=epoch.current;
      if(action==='get')selection.current=projectionId;
      try{
        const response=await fetch('api/sic-core/native-projections',{method:'POST',headers:{'content-type':'application/json'},
          body:JSON.stringify({sessionId,action,offset,limit:100,...cursors,...projectionId?{projectionId}:{}})});
        const result=await response.json();if(!response.ok||!result.ok)throw Error('unavailable');
        if(generation===epoch.current&&(action==='list'||selection.current===projectionId)){setError('');if(action==='list'){setRows(prior=>offset?[...new Map([...prior,...result.value.rows].map(row=>[row.id,row])).values()]:result.value.rows);setNextListOffset(result.value.next_offset);}else if(action==='history')setHistory(prior=>{
          if(part==='all'||!prior)return result.value;
          const page=result.value,merge=(a,b,key)=>[...new Map([...a,...b].map(row=>[row[key],row])).values()];
          if(part==='editor')return{...prior,editor:{...page.editor,rows:[...prior.editor.rows.slice(0,offset),...page.editor.rows,...prior.editor.rows.slice(offset+page.editor.rows.length)]}};
          const core={...prior.core};
          if(part==='versions'){core.versions=merge(core.versions,page.core.versions,'version');core.has_more=page.core.has_more;core.next_after_version=page.core.next_after_version;}
          if(part==='i'){core.i_projections=merge(core.i_projections,page.core.i_projections,'revision');core.has_more_i=page.core.has_more_i;core.next_after_i_revision=page.core.next_after_i_revision;}
          return{...prior,core};
        });else{setRecord(result.value);setHistory(null);}}
      }catch{if(generation===epoch.current)setError('无法读取当前会话的原生上下文投影。');}
    }
    const detail=(title,value)=>h('details',{key:title},h('summary',null,title),h('pre',{style:{whiteSpace:'pre-wrap',overflowWrap:'anywhere',maxHeight:500,overflow:'auto'}},JSON.stringify(value,null,2)));
    return h('div',{style:{padding:20,maxWidth:1100,margin:'auto'}},h('h2',null,'实际采用的原生上下文'),
      h('p',null,'逐项查看来源、文本锚、编辑与实际发送内容。事务提交与 CB 发布分别显示。'),
      h('button',{disabled:!sessionId,onClick:()=>void query('list')},'读取投影'),error?h('p',{role:'alert'},error):null,
      ...rows.map(row=>h('div',{key:row.id},h('button',{onClick:()=>void query('get',row.id)},`${row.format} · ${row.request_id} · ${row.stage}`))),
      nextListOffset!=null?h('button',{onClick:()=>void query('list',undefined,nextListOffset)},'读取下一页请求'):null,
      record?h(React.Fragment,null,h('p',null,`来源 ${record.source_id} · ${record.role??'角色未记录'} · ${record.adopted_digest}`),
        (record.layout_receipt?.image_view??(record.layout_receipt?null:record.host_binding?.context_projection?.image_view))?.lineage?
          h(ImageLineage,{key:record.id,sessionId,projectionId:record.id,view:record.layout_receipt?.image_view??record.host_binding.context_projection.image_view}):null,
        detail('原生原文 / Source',record.original),detail('授权文本锚',record.units),detail('编辑操作 / Candidate',record.edits),detail('候选请求',record.candidate_request),detail('实际采用布局回执',record.layout_receipt),
        h('button',{onClick:()=>void query('history',record.id)},'读取来源版本与发布历史'),history?detail('来源版本与 Editor 流程',history):null,
        history?.core?.has_more?h('button',{onClick:()=>void query('history',record.id,0,'versions')},'读取下一页候选版本'):null,
        history?.core?.has_more_i?h('button',{onClick:()=>void query('history',record.id,0,'i')},'读取下一页 I 刷新'):null,history?.editor?.next_offset!=null?h('button',{onClick:()=>void query('history',record.id,history.editor.next_offset,'editor')},'读取下一页流程'):null,detail('请求与响应观测',record.outcomes??[]),record.observation_errors?.length?detail('观测文件诊断',record.observation_errors):null,detail('事务回执',record.receipt),detail('CB 发布关联',record.publication??{status:'not_bound_to_cb_publication'}),
        detail('实际采用的原生请求',record.adopted),detail('原生原始载体',record.raw_original),detail('实际发送载体',record.raw_adopted),
        detail('请求与响应关联',{request_id:record.request_id,projection_id:record.id,route:record.route})):null);
  }
  return{name:'sic-transparent-host',inject:['slots'],apply(ctx){ctx.effect(()=>ctx.slots.inject('conversation.view',()=>ctx.slots.register(
    {name:'conversation.view',id:'sic-native-context',order:23,label:()=> 'SIC 上下文'},props=>h(NativeTrace,props))), 'sic: native context view');}};
}});
