import { test } from 'node:test';
import assert from 'node:assert/strict';
import { addContact, contactMembership, jobState, payload } from './model.js';
test('recall request and native failure never appear as final success', () => {
  assert.equal(jobState({ status:'sent', recall_status:'requested' }).label, '已请求撤回');
  assert.equal(jobState({ status:'sent', recall_status:'recall_failed' }).tone, 'error');
  assert.equal(jobState({ status:'uncertain' }).label, '待核对');
  assert.equal(jobState({ status:'sent', recall_status:'recalled' }).label, '已撤回');
});
test('whitelist classification never leaks equal names into the other type', () => {
  const original={whitelist:['同名'],group_whitelist:[]};
  const next=addContact(original,{name:'同名',group:true});
  assert.deepEqual(next,{whitelist:['同名'],group_whitelist:['同名']});
  assert.deepEqual(original,{whitelist:['同名'],group_whitelist:[]});
});
test('invalid settings are rejected before an action; recall caps at 120', () => {
  const valid={port:'8765',self_id:'900000000001',interval:'.2',recall_limit:'999'};
  assert.equal(payload(valid).recall_limit,120);
  for(const bad of [{port:0},{port:'oops'},{self_id:'1e20'},{interval:''}]) assert.throws(()=>payload({...valid,...bad}));
});
test('unsaved membership is never presented as an applied whitelist', () => {
  assert.deepEqual(contactMembership({whitelist:['新增']},{whitelist:[]},{name:'新增',group:false}),{applied:false,edited:true});
  assert.deepEqual(contactMembership({whitelist:[]},{whitelist:['待删除']},{name:'待删除',group:false}),{applied:true,edited:false});
});
