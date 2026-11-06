export const presets = {
  custom: {name:'自定义兼容接口',base_url:'',allow_http:false,require_key:true},
  deepseek: {name:'DeepSeek',base_url:'https://api.deepseek.com/v1',allow_http:false,require_key:true},
  qwen: {name:'阿里百炼',base_url:'https://dashscope.aliyuncs.com/compatible-mode/v1',allow_http:false,require_key:true},
  ollama: {name:'Ollama / 本地模型',base_url:'http://127.0.0.1:11434/v1',allow_http:true,require_key:false}
};
export function newProfile(provider='custom',id=`model-${Date.now().toString(36)}`) {
  return {id,provider,...presets[provider],model:'',token_parameter:'max_tokens',temperature:null,system_prompt:'你是一个可靠的中文助手。根据用户问题回答；不确定时明确说明，不声称已执行未执行的操作。',api_key:'',has_key:false};
}
export function aiPayload(draft) {
  const result={...draft,profiles:draft.profiles.map(p=>({...p,temperature:p.temperature===''||p.temperature==null?null:Number(p.temperature)}))};
  for(const [key,min,max] of [['concurrency',1,8],['timeout_seconds',5,180],['context_turns',0,30],['context_minutes',1,1440],['input_budget',512,65536],['max_output_tokens',64,8192],['requests_per_minute',1,600],['daily_token_budget',0,1000000000]]) {
    const n=Number(result[key]);
    if(result[key]===''||!Number.isSafeInteger(n)||n<min||n>max)throw new Error(`AI 参数 ${key} 须为 ${min}–${max} 的整数`);
    result[key]=n;
  }
  if(result.enabled&&!result.profiles.some(p=>p.id===result.active_profile))throw new Error('请选择当前使用的模型配置');
  for(const p of result.profiles) {
    if(!p.name.trim()||!p.model.trim())throw new Error('请填写配置名称和模型 ID');
    let u;try {u=new URL(p.base_url);}catch{throw new Error('请填写完整的模型接口地址');}
    if(!['https:','http:'].includes(u.protocol)||u.username||u.password||u.search||u.hash)throw new Error('模型地址须为 HTTP(S)，且不能带账号、密码、查询参数或片段');
    if(u.protocol==='http:'&&!p.allow_http)throw new Error('本地/内网 HTTP 地址需要开启允许明文连接');
    if(p.temperature!==null&&(!Number.isFinite(p.temperature)||p.temperature<0||p.temperature>2))throw new Error('temperature 须为 0–2，或留空');
  }
  return result;
}
export function aiState(task) {
  return ({queued:['排队中','pending'],running:['生成中','pending'],generated:[task.kind==='probe'?'测试完成':'已生成 · 待提交','success'],awaiting_delivery:['已入发送队列','pending'],delivered:['已确认送达','success'],uncertain:['结果待核对','pending'],failed:['失败','error'],cancelled:['已取消','neutral'],expired:['已过期','neutral']})[task.status]||['未知状态','neutral'];
}
