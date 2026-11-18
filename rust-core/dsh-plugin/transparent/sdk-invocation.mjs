import{AsyncLocalStorage}from'node:async_hooks';
const active=new AsyncLocalStorage();
export const getSdkAdoption=()=>active.getStore();
// Async iterators are lazy: enter the host-only scope on EVERY iterator method,
// rather than returning a generator after its AsyncLocalStorage scope expired.
export function withSdkAdoption(authority,factory){return{[Symbol.asyncIterator](){let iterator;const enter=(name,args)=>active.run(authority,()=>{iterator??=factory()[Symbol.asyncIterator]();return iterator[name]?.(...args)??Promise.resolve({done:true});});return{next:(...args)=>enter('next',args),return:(...args)=>enter('return',args),throw:(...args)=>enter('throw',args),[Symbol.asyncIterator](){return this;}};}};}
