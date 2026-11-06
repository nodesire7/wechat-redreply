import {test} from 'node:test';
import assert from 'node:assert/strict';
import {aiPayload,aiState,newProfile} from './ai-model.js';
const valid=()=>({enabled:true,active_profile:'local',profiles:[{...newProfile('ollama','local'),model:'fixture'}],concurrency:2,timeout_seconds:90,context_turns:6,context_minutes:30,input_budget:8192,max_output_tokens:1024,requests_per_minute:30,daily_token_budget:0});
test('AI uses separate lifecycle states and does not mark generation as delivery',()=>{
  assert.equal(aiState({status:'generated',kind:'chat'})[0],'已生成 · 待提交');
  assert.equal(aiState({status:'awaiting_delivery'})[0],'已入发送队列');
  assert.equal(aiState({status:'delivered'})[0],'已确认送达');
  assert.equal(aiState({status:'uncertain'})[1],'pending');
});
test('saved credential indicators survive editing without inventing a key',()=>{
  const draft=valid();draft.profiles[0].has_key=true;
  const value=aiPayload(draft);assert.equal(value.profiles[0].api_key,'');assert.equal(value.profiles[0].has_key,true);assert.equal(value.profiles[0].temperature,null);
  assert.equal(draft.profiles[0].api_key,'');
});
test('invalid endpoint and numeric edits prevent save and dependent test',()=>{
  for(const changes of [{timeout_seconds:''},{concurrency:0},{context_turns:31},{daily_token_budget:-1},{input_budget:1.5}])assert.throws(()=>aiPayload({...valid(),...changes}));
  for(const base_url of ['file:///secret','https://user:secret@example.com/v1','https://example.com/v1?key=secret']) {const d=valid();d.profiles[0].base_url=base_url;assert.throws(()=>aiPayload(d));}
  const d=valid();d.profiles[0].allow_http=false;assert.throws(()=>aiPayload(d));
});
