// Populate the current TCI exit immediates for synthetic IR execution fixtures.
// Arithmetic, ABI and exit-result expectations stay in the individual drivers.
export type Fn=(env:number,bank:number,scratch:number,codeBase:number)=>bigint;
export function withSource(module:WebAssembly.Module,instance:WebAssembly.Instance,memory:WebAssembly.Memory):Record<string,Fn>{
    const sections=WebAssembly.Module.customSections(module,'tci_test_source');
    if(sections.length!==1)throw Error('missing synthetic TCI source');
    const words=new DataView(sections[0]);
    const raw=instance.exports as Record<string,Fn>;
    return Object.fromEntries(Object.entries(raw).map(([name,fn])=>[name,(env:number,bank:number,scratch:number,base:number)=>{
        const view=new DataView(memory.buffer),index=Number(name.slice(1));
        for(let at=0;at<words.byteLength;at+=12)if(words.getUint32(at,true)===index)view.setUint32(base+words.getUint32(at+4,true),words.getUint32(at+8,true),true);
        return fn(env,bank,scratch,base);
    }]));
}
