export const pages = ['account', 'scope', 'queue', 'logs', 'advanced'];
export const pageNames = ['账号与接入', '接管范围', '发送队列', '日志与对话', '高级设置'];
export function jobState(job) {
  if (job.recall_status === 'recall_failed') return { label: '撤回未全部成功', tone: 'error' };
  if (job.recall_status === 'recalled') return { label: '已撤回', tone: 'neutral' };
  if (job.status === 'cancelled' || job.recall_status === 'cancelled') return { label: '已取消发送', tone: 'neutral' };
  if (job.recall_status === 'recalling') return { label: '正在撤回', tone: 'pending' };
  if (job.recall_status === 'requested') return { label: '已请求撤回', tone: 'pending' };
  if (job.status === 'failed') return { label: '准备失败', tone: 'error' };
  if (job.status === 'uncertain') return { label: '待核对', tone: 'pending' };
  if (job.status === 'sent') return { label: job.recall_status === 'scheduled' ? '已发送 · 定时撤回' : '已确认发送', tone: 'success' };
  return { label: ({ queued: '排队中', preparing: '准备中', sending: '发送中' })[job.status] || '未知状态', tone: 'pending' };
}
export function payload(draft) {
  const port = Number(draft.port), self_id = Number(draft.self_id);
  if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error('端口须为 1–65535 的整数');
  if (!Number.isSafeInteger(self_id) || self_id <= 0) throw new Error('机器人 ID 须为有效的正整数');
  const interval = Number(draft.interval), recall_limit = Number(draft.recall_limit);
  if (!Number.isFinite(interval) || interval < 0.05 || interval > 5) throw new Error('查询间隔须为 0.05–5 秒');
  if (!Number.isFinite(recall_limit) || recall_limit <= 0) throw new Error('撤回上限须大于 0 秒');
  return { ...draft, port, self_id, interval, recall_limit: Math.min(recall_limit, 120) };
}
export function addContact(draft, contact) {
  const key = contact.group ? 'group_whitelist' : 'whitelist';
  return { ...draft, [key]: [...new Set([...(draft[key] || []), contact.name])] };
}
export function contactMembership(draft, saved, contact) {
  const key = contact.group ? 'group_whitelist' : 'whitelist';
  return { applied: (saved?.[key] || []).includes(contact.name), edited: (draft?.[key] || []).includes(contact.name) };
}
