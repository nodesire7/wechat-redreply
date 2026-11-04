import { useCallback, useEffect, useState } from 'react';
import MagicBento from './components/MagicBento.jsx';
import Shredder from './components/Shredder.jsx';
import SpotlightCard from './components/SpotlightCard.jsx';
import StarBorder from './components/StarBorder.jsx';
import CountUp from './components/CountUp.jsx';
import avatar from './avatar.png';

const RED = '#d92d20';
const GREEN = '#07855a';
const token = new URLSearchParams(location.search).get('token') || '';

async function api(path, body) {
  const url = '/api/' + path + (path.includes('?') ? '&' : '?') + 'token=' + encodeURIComponent(token);
  const opt = { cache: 'no-store', credentials: 'same-origin', headers: { 'X-Token': token } };
  if (body !== undefined) {
    opt.method = 'POST';
    opt.headers['Content-Type'] = 'application/json';
    opt.body = JSON.stringify(body);
  }
  const res = await fetch(url, opt);
  const data = await res.json().catch(() => ({ error: 'HTTP ' + res.status }));
  if (!res.ok || data.error) throw new Error(data.error || 'HTTP ' + res.status);
  return data;
}

const sampleQueue = [
  { id: 1, who: '阿飞', kind: '私聊', meta: '封面图 · 400×600 · 285KB', status: 'ok', label: '已确认' },
  { id: 2, who: '资源分享群', kind: '群聊', meta: '合并转发 · 4 个中转项 · 19,001 字', status: 'ok', label: '已确认' },
  { id: 3, who: '阿飞', kind: '私聊', meta: '片名：终极名单 第二季 · 100s 后撤回', status: 'wait', label: '等待撤回' },
  { id: 4, who: '阿飞', kind: '私聊', meta: '输入对应编号查看结果 · 30s 后撤回', status: 'recall', label: '已撤回' },
  { id: 5, who: '资源分享群', kind: '群聊', meta: '资源列表 · magnet × 12 · 排队中', status: 'wait', label: '队列 1/12' }
];


function mapJob(j) {
  const recall = j.recall_status || '';
  let status = 'wait', label = '排队中';
  if (j.status === 'failed') { status = 'recall'; label = '失败'; }
  else if (j.status === 'uncertain') { status = 'wait'; label = '待确认'; }
  else if (recall === 'recalled') { status = 'recall'; label = '已撤回'; }
  else if (recall === 'requested' || recall === 'scheduled' || recall === 'recalling') { status = 'wait'; label = '等待撤回'; }
  else if (j.status === 'sent') { status = 'ok'; label = '已确认'; }
  return {
    id: j.message_id,
    who: j.name,
    kind: j.group ? '群聊' : '私聊',
    meta: `#${j.message_id} · ${j.error || j.status}`,
    status,
    label
  };
}

const nav = [['账号与接入', '1'], ['接控范围', ''], ['发送队列', ''], ['日志与对话', ''], ['高级设置', '']];

const personIcon = (
  <svg viewBox="0 0 24 24"><circle cx="12" cy="7.5" r="3.2" /><path d="M5 20.5v-2.7c0-3 2.4-4.8 7-4.8s7 1.8 7 4.8v2.7" /></svg>
);

function bentoItems(outbox, sent) {
  const n = (k) => Number(outbox[k] || 0);
  const cell = (label, value, tone, suffix) => ({
    content: (
      <div className={`bento-kpi ${tone}`}>
        <span className="bento-kpi__label">{label}</span>
        <span className="bento-kpi__value">{value}<small>{suffix}</small></span>
      </div>
    )
  });
  return [
    cell('QUEUED', n('queued') + n('preparing'), 'red', '条'),
    cell('SENT', sent, 'green', '条'),
    cell('FAILED', n('failed') + n('uncertain'), '', '条'),
    {
      content: (
        <div className="bento-kpi">
          <span className="bento-kpi__label">RECALL WINDOW</span>
          <span className="bento-kpi__value" style={{ fontSize: 20 }}>100<small>s / 120s</small></span>
          <div className="bento-bar"><i style={{ width: '83%' }} /></div>
        </div>
      )
    }
  ];
}

export default function App() {
  const [theme, setTheme] = useState(() => localStorage.getItem('rr-theme') || 'light');
  const [queue, setQueue] = useState([]);
  const [status, setStatus] = useState({});
  const [binding, setBinding] = useState({});
  const [outbox, setOutbox] = useState({});
  const [logs, setLogs] = useState([]);
  const [notice, setNotice] = useState('正在连接…');
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    localStorage.setItem('rr-theme', theme);
  }, [theme]);

  const load = useCallback(async () => {
    try {
      const state = await api('state');
      setStatus(state.status || {});
      setBinding(state.binding || {});
      const st = await api('status');
      setOutbox(st.outbox || {});
      const lg = await api('logs?filter=311');
      setLogs(String(lg.text || '').split('\n').filter(Boolean).slice(-40).reverse());
      const jb = await api('jobs');
      setQueue((jb.jobs || []).map(mapJob));
      setNotice('');
    } catch (e) {
      setNotice(String(e.message || e));
    }
  }, []);

  useEffect(() => {
    load();
    const timer = setInterval(load, 2500);
    return () => clearInterval(timer);
  }, [load]);

  const run = async (action) => {
    setBusy(true);
    try {
      await api(action, {});
      setNotice(action === 'start' ? '监听正在启动…' : '正在停止监听…');
      setTimeout(load, 1200);
    } catch (e) {
      setNotice(String(e.message || e));
    } finally {
      setBusy(false);
    }
  };


  const shredItem = async (item) => {
    try {
      await api('recall', { message_id: item.id });
      setNotice(`已请求撤回 #${item.id}`);
      setQueue((q) => q.filter((x) => x.id !== item.id));
      setTimeout(load, 1500);
    } catch (e) {
      setNotice(String(e.message || e));
      load();
    }
  };

  const sent = Number(outbox.sent || 0);
  const queued = Number(outbox.queued || 0) + Number(outbox.preparing || 0);
  const ready = !!status.ready;
  const running = !!status.running;
  const connected = !!status.connected;

  return (
    <div className="app">
      <header className="titlebar">
        <div className="brand"><img src={avatar} alt="nodesire7" /><span className="brand-name">微信 · 红色问答</span><span className="brand-tag">REACTBITS</span></div>
        <div className="win">
          <button className="wc" onClick={() => setTheme(theme === 'dark' ? 'light' : 'dark')} aria-label="切换明暗">{theme === 'dark' ? '☾' : '☀'}</button>
          <button className="wc" onClick={() => api('window', { action: 'minimize' }).catch(() => {})}>—</button>
          <button className="wc" onClick={() => api('window', { action: 'maximize' }).catch(() => {})}>□</button>
          <button className="wc close" onClick={() => api('window', { action: 'close' }).catch(() => {})}>×</button>
        </div>
      </header>

      <aside className="sidebar">
        <div className="nav-label">CONTROL</div>
        {nav.map(([label, badge], i) => (
          <button key={label} className={`nav-item ${i === 0 ? 'active' : ''}`} onClick={() => { if (i === 1 || i === 4) location.href = '/classic' + location.search; }}>{personIcon}<span>{label}</span>{badge && <b className="badge">{badge}</b>}</button>
        ))}
        <div className="side-foot">
          <div className="userbox"><img src={avatar} alt="" /><div><div className="n">nodesire7</div><div className="s">github.com/nodesire7</div></div></div>
          <div className="mini"><i className={`dot ${binding.valid ? 'on' : ''}`} /><span>微信 {binding.valid ? '已绑定' : '未绑定'}</span></div>
          <div className="mini"><i className={`dot ${connected ? 'on' : ''}`} /><span>RedReply {connected ? '已连接' : '未连接'}</span></div>
          <div className="mini"><i className={`dot ${running ? 'red' : ''}`} /><span>{running ? '服务运行中' : '服务已停止'}</span></div>
        </div>
      </aside>

      <main className="workspace">
        <div className="topline">
          <div><h1>账号与接入</h1><p>{notice || '绑定微信，连接红色问答；队列、撤回与日志在这一层闭环。'}</p></div>
          <div className="chips">
            <span className={`chip ${binding.valid ? 'green' : ''}`}><i className={`dot ${binding.valid ? 'on' : ''}`} />微信 <strong>{binding.valid ? '已绑定' : '未绑定'}</strong></span>
            <span className={`chip ${connected ? 'green' : ''}`}><i className={`dot ${connected ? 'on' : ''}`} />RedReply <strong>{connected ? '已连接' : '未连接'}</strong></span>
            <span className={`chip ${ready ? 'green' : ''}`}>outbox <strong>{ready ? 'durable' : '待就绪'}</strong></span>
          </div>
        </div>

        <section className="grid-main">
          <SpotlightCard className="hero" theme={theme} spotlightColor={RED} intensity={0.12} spotlightSize={320} borderGlow={0.35} flare={false} grain={0.03}>
            <div className="hero-head"><span className="eyebrow">DELIVERY · LAST 24H</span><span className="live"><i />LIVE</span></div>
            <div className="big"><b><CountUp to={sent} duration={1.2} separator="," /></b><span>条已确认送达</span><em>{queued ? `队列 ${queued}` : '队列空闲'}</em></div>
            <div className="hero-foot">
              <div className="hero-bar-wrap"><div className="mini"><span>今日队列</span><span>{sent + queued ? Math.round((sent / (sent + queued)) * 100) : 100}%</span></div><div className="bar"><i style={{ width: `${sent + queued ? Math.round((sent / (sent + queued)) * 100) : 100}%` }} /></div></div>
              <svg className="spark" viewBox="0 0 320 54" preserveAspectRatio="none" aria-hidden="true">
                <defs>
                  <linearGradient id="line" x1="0" y1="0" x2="1" y2="0"><stop offset="0" stopColor={RED} /><stop offset="1" stopColor={GREEN} /></linearGradient>
                  <linearGradient id="area" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stopColor={RED} stopOpacity="0.13" /><stop offset="1" stopColor={RED} stopOpacity="0" /></linearGradient>
                </defs>
                <path fill="url(#area)" d="M0 42 C22 38 34 44 52 36 S84 22 104 28 138 40 158 30 190 12 210 20 246 34 268 24 300 8 320 14 L320 54 L0 54Z" />
                <path fill="none" stroke="url(#line)" strokeWidth="2" strokeLinecap="round" d="M0 42 C22 38 34 44 52 36 S84 22 104 28 138 40 158 30 190 12 210 20 246 34 268 24 300 8 320 14" />
              </svg>
            </div>
            <div className="actions">
              <StarBorder as="button" theme={theme} color={theme === 'dark' ? '#ff7a6b' : RED} trailColor={theme === 'dark' ? '#2fce91' : GREEN} backgroundColor={theme === 'dark' ? '#ff5c4d' : RED} textColor="#fff" thickness={1} radius={10} duration={5} stars={1} glow={0.5} className="primary" onClick={() => run('start')} disabled={busy || !binding.valid}>启动服务</StarBorder>
              <button className="ghost" onClick={() => run('stop')} disabled={busy}>停止</button>
              <a className="ghost" href={'/classic' + location.search}>绑定 / 设置</a>
            </div>
          </SpotlightCard>

          <MagicBento items={bentoItems(outbox, sent)} columns={2} rowHeight={104} gap={10} radius={14} theme={theme} glowColor={RED} enableBorderGlow enableSpotlight spotlightRadius={220} clickEffect textAutoHide={false} intro={false} className="kpi-bento" />
        </section>

        <section className="grid-lower">
          <SpotlightCard className="panel queue-panel" theme={theme} spotlightColor={RED} intensity={0.08} borderGlow={0.25} flare={false} grain={0.02}>
            <div className="panel-head"><div className="panel-title">实时队列 <span className="tag">SHREDDER</span></div><span className="hint">拖入碎纸口移除（示意）</span></div>
            {queue.length ? <Shredder items={queue} renderItem={(item) => (
              <div className="qrow">
                <span className={`qico ${item.status}`}>{item.status === 'ok' ? '◈' : item.status === 'recall' ? 'T' : '≡'}</span>
                <div className="qmain"><div className="qwho">{item.who}<span className="kind">{item.kind}</span></div><div className="qmeta">{item.meta}</div></div>
                <span className={`qst ${item.status}`}>{item.label}</span>
              </div>
            )} onShred={shredItem} width={560} height={430} gap={8} inset={6} fallHeight={96} stripWidth={9} curl={0.8} slitColor={RED} color={theme === 'dark' ? '#f7efef' : '#171313'} /> : <div className="empty">暂无队列任务</div>}
          </SpotlightCard>

          <SpotlightCard className="panel log-panel" theme={theme} spotlightColor={GREEN} intensity={0.08} borderGlow={0.25} flare={false} grain={0.02}>
            <div className="panel-head"><div className="panel-title">日志 <span className="tag">TAIL</span></div><button className="ghost sm" onClick={() => setLogs([])}>清屏</button></div>
            <div className="logbox">
              <div className="logbar"><i /><i /><i /><span>runtime.log</span></div>
              <pre>{logs.length ? logs.map((line, i) => <div key={i}>{line}</div>) : '暂无日志'}</pre>
            </div>
          </SpotlightCard>
        </section>

        <div className="foot"><span>ReactBits 真实组件：MagicBento · Shredder · SpotlightCard · StarBorder · CountUp</span><span>红白绿 · 明暗双模式</span></div>
      </main>
    </div>
  );
}
