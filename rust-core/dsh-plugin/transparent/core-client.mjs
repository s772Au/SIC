export class CoreOperationError extends Error {
  constructor(operation,status,error){
    super(`Core ${operation} rejected: ${error?.code??status}`);
    this.operation=operation;
    this.status=status;
    this.code=error?.code;
    this.detail=error?.message;
  }
}

// Authentication is supplied by the existing host; never reads token files/env.
export function createCoreClient({baseURL,credential,context,fetchImpl=globalThis.fetch,requestTimeoutMs=30_000,operationTimeoutMs={}}){
  const base=new URL(baseURL);
  if(base.username||base.password||base.search||base.hash||!['http:','https:'].includes(base.protocol))throw Error('invalid Core endpoint');
  if(typeof credential!=='function'||typeof context!=='function')throw TypeError('host credential/context functions required');
  if(!Number.isSafeInteger(requestTimeoutMs)||requestTimeoutMs<1||requestTimeoutMs>2_147_483_647)throw TypeError('invalid Core control request timeout');
  const deadlines=new Map(Object.entries(operationTimeoutMs));
  if([...deadlines.values()].some(value=>!Number.isSafeInteger(value)||value<1||value>2_147_483_647))throw TypeError('invalid Core operation timeout');
  // This deadline applies only to local Core control RPCs, including response
  // bodies. It does not impose a Provider/model timeout or retry a mutation.
  async function request(path,body,signal,operation){
    const timeoutMs=deadlines.get(operation)??requestTimeoutMs;
    const timeout=new DOMException(`Core ${operation} control request timed out after ${timeoutMs} ms`,'TimeoutError');
    const deadline=new AbortController(),timer=setTimeout(()=>deadline.abort(timeout),timeoutMs);timer.unref?.();
    try{
      const response=await fetchImpl(new URL(path,base),{method:'POST',redirect:'error',signal:signal?AbortSignal.any([signal,deadline.signal]):deadline.signal,
        headers:{authorization:`Bearer ${credential()}`,'content-type':'application/json','cache-control':'no-store'},body});
      return{response,value:await response.json()};
    }catch(error){
      if(deadline.signal.aborted&&!signal?.aborted)throw timeout;
      throw error;
    }finally{clearTimeout(timer);}
  }
  async function operate(operation,payload,signal,rawPayload){
    const ctx=context();
    if(!ctx?.actor)throw Error('host actor required');
    const {response,value}=await request('/api/v2/operations',rawPayload===undefined?JSON.stringify({operation,context:ctx,payload})
        :JSON.stringify({operation,context:ctx}).slice(0,-1)+',"payload":'+rawPayload+'}',signal,operation);
    if(!response.ok)throw new CoreOperationError(operation,response.status,value?.error);
    if(!Object.hasOwn(value,'result'))throw Error('invalid Core operation envelope');
    return value.result;
  }
  async function issueTurn(metadata,signal){
    const {response,value}=await request('/api/v2/harness/turn',JSON.stringify(metadata),signal,'harness.turn');
    if(!response.ok||typeof value.grant_id!=='string')throw Error('host turn grant rejected');return value;
  }
  return{operate,issueTurn,begin:(p,s)=>operate('native.begin',p,s),
    beginRaw:(format,raw,signal,scope={})=>{JSON.parse(raw);const fields=JSON.stringify(scope).slice(1,-1);return operate('native.begin',null,signal,'{"format":'+JSON.stringify(format)+',"request":'+raw+(fields?','+fields:'')+'}');},
    commit:(p,s)=>operate('native.commit',p,s),get:(p,s)=>operate('native.get',p,s),publish:(p,s)=>operate('native.publish',p,s),history:(p,s)=>operate('native.history',p,s),preview:(p,s)=>operate('native.preview',p,s),refreshI:(p,s)=>operate('native.refresh_i',p,s)};
}
