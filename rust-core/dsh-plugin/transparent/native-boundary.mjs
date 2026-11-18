// Mirror of the monotonic boundary added to the reviewed native Provider host.
// Guards live in a closure and cannot be removed by a later waterfall plugin.
export function nativeExchange(request,provenance,format='anthropic-messages'){
  const guards=[],exchange={request,provenance,format};
  Object.defineProperty(exchange,'addFinalGuard',{value:guard=>{
    if(typeof guard!=='function')throw TypeError('native final guard must be callable');guards.push(guard);
  },writable:false,configurable:false,enumerable:true});
  Object.preventExtensions(exchange);
  return{exchange,dispatch:async fetchImpl=>{for(const guard of guards)await guard(exchange.request,exchange.provenance);return fetchImpl(exchange.request);}};
}
