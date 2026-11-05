import { useCallback, useEffect, useRef, useState } from 'react';
import CountUp from './components/CountUp.jsx';
import SpotlightCard from './components/SpotlightCard.jsx';
import StarBorder from './components/StarBorder.jsx';
import MagicBento from './components/MagicBento.jsx';
import avatar from './avatar.png';
import { addContact, contactMembership, jobState, pages, pageNames, payload } from './model.js';

const token = new URLSearchParams(location.search).get('token') || '';
async function api(path, body) {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), ['save','bind','stop'].includes(path) ? 45000 : 12000);
  const opt = { cache: 'no-store', credentials: 'same-origin', signal: controller.signal, headers: {} };
  if (token) opt.headers['X-Token'] = token;
  if (body !== undefined) { opt.method = 'POST'; opt.headers['Content-Type'] = 'application/json'; opt.body = JSON.stringify(body); }
  try {
    const res = await fetch('/api/' + path, opt);
    const data = await res.json();
    if (!res.ok || data.error) throw new Error(data.error || `请求失败（${res.status}）`);
    return data;
  } catch (error) { throw new Error(error.name === 'AbortError' ? '请求超时，请检查程序是否仍在运行' : error.message); }
  finally { clearTimeout(timeout); }
}

function Icon({ name, ...props }) {
  const paths = {
    account: <><circle cx="12" cy="7" r="3"/><path d="M5 21v-3c0-4 14-4 14 0v3"/></>,
    scope: <><circle cx="8" cy="7" r="3"/><circle cx="18" cy="9" r="2"/><path d="M2 21v-3c0-5 12-5 12 0v3M17 15c4 0 5 2 5 6"/></>,
    queue: <><path d="M4 5h16M4 12h16M4 19h10"/><path d="m17 17 3 3-3 3"/></>,
    logs: <><rect x="4" y="3" width="16" height="18" rx="2"/><path d="M8 8h8M8 12h8M8 16h5"/></>,
    advanced: <><circle cx="12" cy="12" r="4"/><path d="M12 2v3M12 19v3M2 12h3M19 12h3M5 5l2 2M17 17l2 2M19 5l-2 2M7 17l-2 2"/></>,
    moon: <path d="M20 14A8 8 0 0 1 10 4a8 8 0 1 0 10 10Z"/>,
    sun: <><circle cx="12" cy="12" r="4"/><path d="M12 1v2M12 21v2M1 12h2M21 12h2M4 4l2 2M18 18l2 2M20 4l-2 2M6 18l-2 2"/></>,
    minus: <path d="M5 12h14"/>, maximize: <rect x="5" y="5" width="14" height="14" rx="1"/>, close: <path d="m6 6 12 12M18 6 6 18"/>,
  };
  return <svg viewBox="0 0 24 24" aria-hidden="true" {...props}>{paths[name]}</svg>;
}
function Field({ label, children, hint, className = '' }) { return <label className={`field ${className}`}><span>{label}</span>{children}{hint && <small>{hint}</small>}</label>; }
function State({ good, children }) { return <span className="status"><i className={good ? 'dot on' : 'dot'}/>{children}</span>; }
function PrimaryButton({theme,children,...props}) {
  return <StarBorder as="button" className="primary" theme={theme} backgroundColor="var(--red)" textColor={theme==='dark'?'#2d1511':'#ffffff'} color={theme==='dark'?'#65d8a5':'#a6efcf'} trailColor={theme==='dark'?'#ff8f80':'#ffd7cf'} duration={6} radius={7} thickness={1.2} glow={0.4} hover="lap" {...props}>{children}</StarBorder>;
}
const time = (value) => value ? new Date(value * 1000).toLocaleString('zh-CN', { hour12: false }) : '—';

export default function App() {
  const [theme, setTheme] = useState(() => localStorage.getItem('rr-theme') || (matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'));
  const [page, setPage] = useState(() => { const q = new URLSearchParams(location.search).get('page'); return pages.includes(q) ? q : 'account'; });
  const [draft, setDraft] = useState(null), [dirty, setDirty] = useState(false);
  const [saved, setSaved] = useState(null), [statusKnown,setStatusKnown] = useState(false);
  const [status, setStatus] = useState({}), [binding, setBinding] = useState({}), [outbox, setOutbox] = useState(null), [statsError,setStatsError] = useState('');
  const [version, setVersion] = useState(''), [notice, setNotice] = useState(''), [error, setError] = useState('');
  const [pollError, setPollError] = useState('');
  const [busy, setBusy] = useState(''), busyRef = useRef(false), polling = useRef(false);
  const [windows, setWindows] = useState([]), [windowId, setWindowId] = useState('');
  const [contacts, setContacts] = useState(null), [scope, setScope] = useState(false), [search, setSearch] = useState('');
  const [manualName,setManualName]=useState('');
  const [capabilities,setCapabilities]=useState(null),[capabilityError,setCapabilityError]=useState('');
  const [jobs, setJobs] = useState([]), [jobFilter, setJobFilter] = useState('all'), [queueError, setQueueError] = useState('');
  const [logs, setLogs] = useState(''), [filter, setFilter] = useState(311), [paused, setPaused] = useState(false), logRef = useRef(null);
  const contactVersion=useRef(0);
  const logVersion=useRef(0), [logFilter,setLogFilter]=useState(null), [logLoading,setLogLoading]=useState(false), [logError,setLogError]=useState('');
  const [showToken, setShowToken] = useState(false);
  const running = status.running === true, connected = running && status.connected === true, ready = running && status.ready === true;

  useEffect(() => { document.documentElement.dataset.theme = theme; localStorage.setItem('rr-theme', theme); }, [theme]);
  const refreshWindows = useCallback(async () => {
    const data = await api('windows'); setWindows(data.windows || []);
    setWindowId((previous) => {
      const selected = (data.windows || []).find(w => String(w.hwnd) === previous) || (data.windows || []).find(w => w.hwnd === data.bound?.hwnd && w.pid === data.bound?.pid);
      return selected ? String(selected.hwnd) : data.windows?.length === 1 ? String(data.windows[0].hwnd) : '';
    });
  }, []);
  const load = useCallback(async () => {
    const data = await api('state'); setDraft(data.config); setSaved(data.config); setBinding(data.binding || {}); setStatus(data.status || {}); setStatusKnown(true); setDirty(false);
    await refreshWindows();
  }, [refreshWindows]);
  const loadJobs = useCallback(async () => {
    try { const data = await api('jobs'); setJobs(data.jobs || []); setQueueError(''); }
    catch (e) { setQueueError(e.message); }
  }, []);
  const loadLogs = useCallback(async () => {
    const sequence=++logVersion.current; setLogLoading(true);
    const view = logRef.current, bottom = !view || view.scrollHeight - view.clientHeight - view.scrollTop < 20, top = view?.scrollTop;
    try {
      const data = await api('logs?filter=' + filter);
      if(sequence!==logVersion.current)return;
      setLogs(data.text || '');setLogFilter(filter);setLogError('');
      requestAnimationFrame(() => { if (logRef.current) logRef.current.scrollTop = bottom ? logRef.current.scrollHeight : top; });
    } catch(e) { if(sequence===logVersion.current)setLogError(e.message); }
    finally { if(sequence===logVersion.current)setLogLoading(false); }
  }, [filter]);
  const poll = useCallback(async () => {
    if (polling.current) return; polling.current = true;
    try {
      const data = await api('status'); setStatus(data.status || {}); setBinding(data.binding || {}); setStatusKnown(true); setOutbox(data.outbox ?? null); setStatsError(data.outbox_error || ''); setVersion(data.version || ''); setPollError('');
      if (page === 'queue') await loadJobs();
      if (page === 'logs' && !paused) await loadLogs();
    } catch (e) { setPollError(`无法刷新运行状态：${e.message}。页面保留的是上次结果。`); }
    finally { polling.current = false; }
  }, [page, paused, loadJobs, loadLogs]);
  useEffect(() => { load().catch(e => setError(e.message)); }, [load]);
  useEffect(() => { poll(); const timer = setInterval(poll, 2000); return () => clearInterval(timer); }, [poll]);
  useEffect(() => { if(page==='logs')loadLogs(); }, [page,loadLogs]);
  useEffect(() => { contactVersion.current++;setContacts(null);setSearch(''); }, [binding.pid,binding.hwnd,binding.created_at]);
  useEffect(() => { if(page==='advanced')api('capabilities').then(setCapabilities).catch(e=>setCapabilityError(e.message)); }, [page]);
  const change = (key, value) => { setDraft(d => ({ ...d, [key]: value })); setDirty(true); };
  const act = async (name, fn) => {
    if (busyRef.current) return; busyRef.current = true; setBusy(name); setError(''); setNotice('');
    try { await fn(); } catch (e) { setError(e.message); }
    finally { busyRef.current = false; setBusy(''); }
  };
  const save = async (next = draft) => {
    const data = await api('save', payload(next)); setDraft(data.config); setSaved(data.config); setBinding(data.binding || {}); setDirty(false); setNotice(data.notice || '设置已保存'); return data;
  };
  const copy = async (text, message) => { await api('copy', { text }); setNotice(message); };
  const windowAction = (action) => act('窗口操作', () => api('window', { action }));
  const input = (key, type = 'text', extra = {}) => <input type={type} value={draft?.[key] ?? ''} onChange={e => change(key, e.target.value)} disabled={!!busy} {...extra}/>;
  const pageTitle = pageNames[pages.indexOf(page)];
  const pageHint = ({account:'绑定微信并复制 OneBot 地址，即可连接现有 RedReply。',scope:'只处理白名单中的会话，私聊与群聊分开保存。',queue:'已入队不等于已发出；待核对任务不会自动重发。',logs:'按对话、协议和运行状态查看实际记录。',advanced:'只对显式指定的输出执行撤回，最长不超过微信的两分钟限制。'})[page];

  return <div className="app">
    <header className="titlebar">
      <div className="brand" onMouseDown={e => { if (e.button === 0) api('window', { action: e.detail === 2 ? 'maximize' : 'drag' }).catch(e => setError(e.message)); }}><img className="brand-avatar" src={avatar} alt=""/><strong>微信 · 红色问答</strong></div>
      <div className="window-controls"><button aria-label="切换明暗主题" onClick={() => setTheme(theme === 'light' ? 'dark' : 'light')}><Icon name={theme === 'light' ? 'moon' : 'sun'}/></button><button aria-label="最小化窗口" onClick={() => windowAction('minimize')}><Icon name="minus"/></button><button aria-label="最大化或还原窗口" onClick={() => windowAction('maximize')}><Icon name="maximize"/></button><button className="window-close" disabled={!!busy} aria-label="关闭设置窗口" onClick={() => { if (!dirty || confirm('有未保存修改。关闭设置窗口并放弃这些修改？')) windowAction('close'); }}><Icon name="close"/></button></div>
    </header>
    <aside className="sidebar"><nav aria-label="工作页面">{pages.map((id, i) => <button key={id} aria-label={pageNames[i]} aria-current={page === id ? 'page' : undefined} onClick={() => { setPage(id); setNotice(''); }}><Icon name={id}/><span>{pageNames[i]}</span></button>)}</nav><div className="sidebar-status"><State good={running}>{!statusKnown ? '正在读取监听状态' : running ? '监听运行中' : '监听已停止'}</State><small>{version ? `Rust v${version}` : '正在读取版本'}</small></div></aside>
    <main><div className="page-heading"><h1>{pageTitle}</h1><div className="status-row"><State good={!!binding.valid}>微信 {!statusKnown ? '读取中' : binding.valid ? '已绑定' : '未绑定'}</State><State good={connected}>RedReply {!statusKnown ? '读取中' : connected ? '已连接' : '未连接'}</State></div></div>
      <div className="feedback" aria-live="polite">{(error || pollError) ? <p role="alert" className="error">{error || pollError}</p> : notice ? <p className="success">{notice}</p> : status.error && running ? <p className="error">{status.error}</p> : <p>{dirty ? '有未保存修改，保存后应用。' : running ? ready ? '微信监听已就绪。关闭设置窗口后继续运行。' : '监听正在准备微信数据库…' : pageHint}</p>}</div>
      {!draft ? <section className="loading"><p>正在读取配置…</p><button disabled={!!busy} onClick={() => act('重新读取', load)}>重新读取</button></section> : <div className="page-body">
        {page === 'account' && <>
          <SpotlightCard className="account-strip" theme={theme} spotlightColor={theme === 'dark' ? '#65d8a5' : '#bd2b20'} intensity={0.08} borderGlow={0.6} flare={false} grain={0} style={{backgroundColor:'var(--panel)',boxShadow:'none'}} aria-label="微信账号绑定"><div className="account-identity"><Icon name="account"/><div><h2>微信账号</h2><p>{binding.label || '未绑定'}</p>{binding.reason && <small className="error">{binding.reason}</small>}</div></div><div className="binding-form"><label htmlFor="wechat-window">已登录的微信窗口</label><div className="binding-actions"><select id="wechat-window" value={windowId} onChange={e => setWindowId(e.target.value)} disabled={!!busy}><option value="">{windows.length ? '选择要接管的微信窗口' : '未发现微信窗口，请登录后刷新'}</option>{windows.map(w => <option key={w.hwnd} value={String(w.hwnd)}>{w.title} · PID {w.pid}{w.offscreen ? ' · 已离屏' : ''}</option>)}</select><button disabled={!!busy} onClick={() => act('刷新窗口', refreshWindows)}>刷新</button><PrimaryButton theme={theme} disabled={!!busy || !windowId} onClick={() => act('绑定账号', async () => { const w = windows.find(w => String(w.hwnd) === windowId); if (!w) throw new Error('窗口已变化，请刷新后选择'); if (dirty) await save(); const data = await api('bind', w); setBinding(data.binding); setDraft(data.config); setSaved(data.config); setNotice(data.notice); await refreshWindows(); })}>绑定账号</PrimaryButton></div><small>只接管所选账号；绑定后窗口自动移到屏幕外。</small></div></SpotlightCard>
          <MagicBento className="connection-bento" theme={theme} columns={3} rowHeight={80} radius={12} gap={12} glowColor={theme === 'dark' ? '#65d8a5' : '#bd2b20'} enableBorderGlow enableSpotlight enableTilt={false} intro={false} textAutoHide={false} items={[{content:<div className="bento-state"><Icon name="account"/><div><span>账号绑定</span><strong>{binding.valid ? '已绑定' : '待绑定'}</strong></div></div>},{content:<div className="bento-state"><Icon name="queue"/><div><span>数据库监听</span><strong>{ready ? '已就绪' : running ? '准备中' : '已停止'}</strong></div></div>},{content:<div className="bento-state"><Icon name="scope"/><div><span>接管范围</span><strong>{(saved?.whitelist || []).filter(Boolean).length} 位好友 · {(saved?.group_whitelist || []).filter(Boolean).length} 个群</strong></div></div>}]}/>
          <section className="connection"><h2>RedReply 连接</h2><div className="form-grid"><Field label="监听 IP" hint="本机与远程接入通常使用 0.0.0.0">{input('listen_host')}</Field><Field label="RedReply 连接此地址" hint="远程填写这台微信机器可达的 IP">{input('connect_host')}</Field><Field label="OneBot 端口">{input('port', 'number', { min: 1, max: 65535 })}</Field><Field label="机器人 ID">{input('self_id', 'text', { inputMode: 'numeric' })}</Field><Field label="接入令牌" className="span2"><div className="token-field">{input('access_token', showToken ? 'text' : 'password', { autoComplete: 'off' })}<button aria-label={showToken ? '隐藏令牌' : '显示令牌'} aria-pressed={showToken} onClick={() => setShowToken(!showToken)}>{showToken ? '隐藏' : '显示'}</button></div></Field><Field label="RedReply 管理地址（可选）" className="span2">{input('management_url', 'url', { placeholder: 'http://服务器地址:1207' })}</Field><div className="field copy-field"><span>接入配置</span><button disabled={!!busy} onClick={() => act('复制接入地址', async () => { const data = await save(); await copy(data.config.url, '完整 OneBot 地址已复制，请填写到 RedReply 的连接平台协议。'); })}>复制 OneBot 地址</button></div></div><details className="wss"><summary>WSS 证书与私钥（可选）</summary><div className="two-columns"><Field label="证书路径">{input('tls_certfile')}</Field><Field label="私钥路径">{input('tls_keyfile')}</Field></div></details></section>
        </>}
        {page === 'scope' && <>
          <div className="toolbar"><div className="segmented" aria-label="联系人类型">{[false, true].map(group => <button key={String(group)} aria-pressed={scope === group} onClick={() => { setScope(group); setSearch(''); setManualName(''); }}>{group ? '群聊' : '私聊'}</button>)}</div><input className="search" aria-label="搜索联系人" placeholder={scope ? '搜索完整群名' : '搜索好友备注名'} value={search} onChange={e => setSearch(e.target.value)}/><button disabled={!!busy || !binding.valid} onClick={() => act('读取联系人', async () => { const sequence=contactVersion.current; const data = await api('contacts', {}); if(sequence!==contactVersion.current)throw new Error('绑定账号已变更，请重新读取联系人'); setContacts(data.contacts || []); setNotice(`已读取 ${data.contacts?.length || 0} 个会话`); })}>{busy === '读取联系人' ? '正在读取…' : '读取联系人'}</button></div>
          <div className="scope-layout"><section><h2>{scope ? '可选群聊' : '可选好友'}</h2><div className="contact-list">{contacts === null ? <p className="empty">绑定微信后点击“读取联系人”，也可在右侧直接添加完整名称。</p> : contacts.filter(c => c.group === scope && c.name.toLocaleLowerCase().includes(search.toLocaleLowerCase())).slice(0, 150).map(c => <div className="contact" key={`${c.group}:${c.name}`}><span title={c.name}>{c.name}</span><button disabled={!!busy || (contactMembership(draft,saved,c).applied && contactMembership(draft,saved,c).edited)} onClick={() => act('加入白名单', () => save(addContact(draft, c)))}>{(contactMembership(draft,saved,c).applied && contactMembership(draft,saved,c).edited) ? '已接管' : contactMembership(draft,saved,c).edited ? '保存接管' : contactMembership(draft,saved,c).applied ? '保留接管' : '加入'}</button></div>)}{contacts !== null && !contacts.some(c => c.group === scope && c.name.toLocaleLowerCase().includes(search.toLocaleLowerCase())) && <p className="empty">未找到匹配的{scope ? '群聊' : '好友'}。</p>}</div><small>最多显示 150 条匹配结果；输入名称缩小范围。</small></section><section><h2>{scope ? '群聊白名单' : '私聊白名单'} <small>已应用 {(saved?.[scope ? 'group_whitelist' : 'whitelist'] || []).length} 个{dirty ? ' · 有待保存编辑' : ''}</small></h2><div className="selected-list">{(saved?.[scope ? 'group_whitelist' : 'whitelist'] || []).map(name => <div className="contact" key={name}><span>{name}</span><button disabled={!!busy} onClick={() => act('移出白名单', () => save({...draft,[scope ? 'group_whitelist' : 'whitelist']:(draft[scope ? 'group_whitelist' : 'whitelist'] || []).filter(value => value.trim() !== name)}))}>移出</button></div>)}{!(saved?.[scope ? 'group_whitelist' : 'whitelist'] || []).length && <p className="empty">尚未接管{scope ? '群聊' : '私聊'}。</p>}</div><div className="manual-add"><input aria-label="手动添加完整名称" placeholder={scope ? '填写完整群名' : '填写完整好友备注名'} value={manualName} onChange={e => setManualName(e.target.value)} disabled={!!busy}/><button disabled={!!busy || !manualName.trim()} onClick={() => act('加入白名单',async () => { await save(addContact(draft,{name:manualName.trim(),group:scope}));setManualName(''); })}>加入</button></div><details className="bulk-edit"><summary>批量编辑完整名称</summary><Field label="每行一个完整名称" hint="空列表表示不接管该类会话；手动修改后点击保存。"><textarea rows="5" value={(draft[scope ? 'group_whitelist' : 'whitelist'] || []).join('\n')} onChange={e => change(scope ? 'group_whitelist' : 'whitelist', e.target.value.split('\n'))} spellCheck="false" disabled={!!busy}/></Field></details></section></div>
        </>}
        {page === 'queue' && <>
          <div className="queue-summary" aria-label="实际任务统计">{[['待发送', outbox ? Number(outbox.queued || 0) + Number(outbox.preparing || 0) + Number(outbox.sending || 0) : null], ['累计发送', outbox ? outbox.sent || 0 : null], ['准备失败', outbox ? outbox.failed || 0 : null], ['待核对', outbox ? outbox.uncertain || 0 : null]].map(([label, count]) => <span key={label}>{label} <strong>{count === null ? '—' : <CountUp to={Number(count)} duration={0.3}/>}</strong></span>)}</div>{statsError && <p role="alert" className="error">{statsError}；统计不代表零任务。</p>}<div className="toolbar"><select aria-label="筛选任务状态" value={jobFilter} onChange={e => setJobFilter(e.target.value)}><option value="all">全部状态</option><option value="pending">待处理</option><option value="problem">失败与待核对</option><option value="recalled">已撤回</option></select><span className="muted">最近 200 条 · 已入队不等于已发出</span><button disabled={!!busy} onClick={() => act('刷新队列', loadJobs)}>刷新</button></div>{queueError && <p className="error" role="alert">队列读取失败：{queueError}</p>}<div className="table-scroll"><table><thead><tr><th>会话 / 消息 ID</th><th>发送与撤回状态</th><th>入队时间</th><th>操作</th></tr></thead><tbody>{jobs.filter(j => jobFilter === 'all' || (jobFilter === 'problem' ? ['failed', 'uncertain'].includes(j.status) || j.recall_status === 'recall_failed' : jobFilter === 'recalled' ? j.recall_status === 'recalled' : ['queued', 'preparing', 'sending'].includes(j.status))).map(j => { const state = jobState(j); return <tr key={j.message_id}><td><strong>{j.name}</strong> <span className="kind">{j.group ? '群聊' : '私聊'}</span><small>#{j.message_id}{j.physical_messages ? ` · ${j.physical_messages} 条微信消息` : ''}</small></td><td><span className={`label ${state.tone}`}>{state.label}</span>{j.error && <small className="error">{j.error}</small>}{j.recall_error && <small className="error">{j.recall_error}</small>}{j.recall_deadline && j.recall_status !== 'recalled' && <small>{j.recall_deadline * 1000 > Date.now() ? '撤回截止 ' + time(j.recall_deadline) : '已超过微信撤回期限'}</small>}</td><td>{time(j.enqueued)}</td><td><button disabled={!!busy || !j.can_recall} onClick={() => act('提交撤回', async () => { await api('recall', { message_id: j.message_id }); setNotice(running ? '请求已登记，等待微信确认；任务会保留显示最终结果。' : '请求已登记，启动监听后执行。'); await loadJobs(); })}>{['queued', 'preparing'].includes(j.status) ? '取消发送' : '撤回整条输出'}</button></td></tr>; })}</tbody></table>{!jobs.length && !queueError && <p className="empty">还没有发送任务。接入 RedReply 并触发白名单会话后，实际结果会显示在这里。</p>}</div>
        </>}
        {page === 'logs' && <><div className="toolbar"><div className="segmented">{[[311, '全部'], [312, '对话'], [313, '协议'], [314, '运行']].map(([id, name]) => <button key={id} aria-pressed={filter === id} onClick={() => setFilter(id)}>{name}</button>)}</div><button aria-pressed={paused} onClick={() => setPaused(!paused)}>{paused ? '继续刷新' : '暂停刷新'}</button><button disabled={!logs || !!busy || logLoading || logFilter !== filter} onClick={() => act('复制日志', () => copy(logs, '当前显示的日志已复制。'))}>复制当前日志</button><span className="muted">{paused ? '已暂停刷新' : '每 2 秒刷新'}</span></div>{logError && <p className="error" role="alert">日志读取失败：{logError}</p>}<pre ref={logRef} className="logs" tabIndex="0" aria-label="收发与运行日志">{logFilter !== filter ? (logError ? '当前分类尚未载入，未显示其他分类记录。' : '正在读取当前分类…') : logs || '尚无此类日志。新消息和操作结果会显示在这里。'}</pre></>}
        {page === 'advanced' && <><section><h2>收发与撤回</h2><div className="two-columns"><Field label="数据库查询间隔（秒）" hint="范围 0.05–5 秒；接收与发送独立运行。">{input('interval', 'number', { min: 0.05, max: 5, step: 0.05 })}</Field><Field label="撤回上限（秒）" hint="最多 120 秒，按每条微信消息的实际发送时间计算。">{input('recall_limit', 'number', { min: 1, max: 120 })}</Field><Field label="合并转发中转自聊" hint="留空时按本账号身份自动识别。">{input('forward_staging_chat', 'text', { placeholder: '自动识别本账号自聊' })}</Field><div className="field"><span>音乐卡片</span><p className="muted">按本次歌曲页面、音频和 QQ 曲目标识生成微信音乐卡；播放需要音源提供有效授权。</p></div></div></section><section className="friend-settings"><h2>好友申请</h2><label className="check"><input type="checkbox" checked={!!draft.friend_accept} onChange={e => change('friend_accept', e.target.checked)} disabled={!!busy}/>自动同意好友申请，权限仅聊天</label><p className="muted">新好友不会自动加入白名单。账号安全验证需在微信内手动完成。</p><button disabled={!!busy} onClick={() => act('重试好友申请', async () => { if (dirty) await save(); const data = await api('retry-friends', {}); setNotice(data.notice); })}>安全验证后重试</button></section><p className="runtime-note">离屏收发始终启用。自动收发不占用剪贴板、鼠标或前台焦点。艾特显示为正文。语音目前支持微信4.2.0.12，超过60秒自动分段；视频、文件及 QQ 管理动作尚未支持。</p><details className="capability-panel"><summary>兼容层支持状态与待对齐动作</summary>{capabilityError && <p className="error">{capabilityError}</p>}{capabilities ? <><table><thead><tr><th>RedLang / OneBot 内容</th><th>实际行为</th><th>状态</th></tr></thead><tbody>{capabilities.mappings.map(row=><tr key={row.operation}><td>{row.operation}</td><td>{row.result}</td><td>{row.state}</td></tr>)}</tbody></table><p className="muted">未对齐动作：{capabilities.unsupported_actions.join('、')}</p></> : <p className="muted">正在读取实际兼容能力…</p>}</details></>}
      </div>}
      <footer><div>{page === 'account' && <button className={running ? '' : 'primary'} disabled={!!busy || (!binding.valid && !running)} onClick={() => act(running ? '停止监听' : '启动监听', async () => { if (!running) await save(); const data = await api(running ? 'stop' : 'start', {}); setNotice(data.notice); await poll(); })}>{busy === '启动监听' || busy === '停止监听' ? '正在处理…' : running ? '停止监听' : '保存并启动监听'}</button>}<span className="muted">{busy ? busy + '…' : dirty ? '未保存' : '配置已载入'}</span></div>{['account', 'scope', 'advanced'].includes(page) && <PrimaryButton theme={theme} disabled={!!busy || !draft} onClick={() => act('保存设置', () => save())}>保存并应用</PrimaryButton>}</footer>
    </main>
  </div>;
}
