// DONGSI_MEDIA_TRACE 폴더(video.csv, audio.csv, publish.csv)를 읽어 마크다운 요약을 만든다.
//
//   node e2e/lib/trace-summary.mjs <폴더>        (apps/desktop에서)
//
// 하네스에서는 import해서 쓴다: summarizeDir(dir) → { markdown, data }, summarize({video, audio, publish}).
// 의존성 없음. CSV 형식은 crates/media/src/trace.rs(video·audio), crates/net/src/publish.rs(publish)의 머리 문서.

import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const QPC = 1e7; // 1초 = 10^7 (100ns 단위)

// ───────────────────────── 읽기 ─────────────────────────

/** CSV 글을 { rows, marks }로. 첫 줄이 머리글, '#'로 시작하는 줄은 구분 줄(새 세그먼트). 빈 칸은 null. */
export function parseCsv(text) {
  const lines = text.split(/\r?\n/).filter((l) => l.length > 0);
  if (lines.length === 0) return { rows: [], marks: [] };
  const header = lines[0].split(',');
  const rows = [];
  const marks = [];
  let seg = 0;
  for (const line of lines.slice(1)) {
    if (line.startsWith('#')) {
      seg += 1;
      marks.push({ seg, text: line, fields: parseMark(line) });
      continue;
    }
    // 머리글이 다시 나오면(파일을 이어 붙인 경우) 건너뛴다.
    if (line === lines[0]) continue;
    const cells = line.split(',');
    const row = { seg };
    header.forEach((h, i) => {
      const v = cells[i];
      row[h] = v === undefined || v === '' ? null : v;
    });
    rows.push(row);
  }
  return { rows, marks };
}

function parseMark(line) {
  const out = { label: '' };
  const toks = line.replace(/^#\s*/, '').split(/\s+/);
  out.label = toks.shift() ?? '';
  for (const t of toks) {
    const i = t.indexOf('=');
    if (i > 0) out[t.slice(0, i)] = t.slice(i + 1);
  }
  return out;
}

const num = (v) => (v === null || v === undefined ? null : Number(v));

/** 폴더에서 있는 파일만 읽는다. */
export function readDir(dir) {
  const read = (name) => {
    const p = path.join(dir, name);
    return fs.existsSync(p) ? parseCsv(fs.readFileSync(p, 'utf8')) : null;
  };
  return { video: read('video.csv'), audio: read('audio.csv'), publish: read('publish.csv') };
}

// ───────────────────────── 통계 도우미 ─────────────────────────

export function stats(values) {
  const v = values.filter((x) => Number.isFinite(x)).sort((a, b) => a - b);
  if (v.length === 0) return { n: 0, avg: NaN, p50: NaN, p95: NaN, max: NaN, min: NaN };
  const q = (p) => v[Math.min(v.length - 1, Math.floor(p * (v.length - 1) + 0.5))];
  const sum = v.reduce((a, b) => a + b, 0);
  return { n: v.length, avg: sum / v.length, p50: q(0.5), p95: q(0.95), max: v[v.length - 1], min: v[0] };
}

const f1 = (x) => (Number.isFinite(x) ? x.toFixed(1) : '-');
const f2 = (x) => (Number.isFinite(x) ? x.toFixed(2) : '-');
const pct = (a, b) => (b > 0 ? ((100 * a) / b).toFixed(1) + '%' : '-');
const ms100 = (d) => d / 1e4; // 100ns → ms

function table(headers, rows) {
  if (rows.length === 0) return '(자료 없음)\n';
  const esc = (c) => String(c).replaceAll('|', '\\|');
  return (
    '| ' + headers.map(esc).join(' | ') + ' |\n' +
    '|' + headers.map(() => '---').join('|') + '|\n' +
    rows.map((r) => '| ' + r.map(esc).join(' | ') + ' |').join('\n') + '\n'
  );
}

function groupBy(items, keyFn) {
  const m = new Map();
  for (const it of items) {
    const k = keyFn(it);
    if (!m.has(k)) m.set(k, []);
    m.get(k).push(it);
  }
  return m;
}

// ───────────────────────── 비디오 ─────────────────────────

/**
 * video.csv 행들을 인코더·크기별로 요약한다. 행은 출력 순서다(파일 순서).
 * 'lost'(출력 없이 버려진 입력)는 지연·순서 통계에서 빼고 개수만 센다.
 */
export function summarizeVideo(parsed) {
  const out = [];
  if (!parsed) return out;
  const groups = groupBy(parsed.rows, (r) => `${r.encoder}|${r.w}x${r.h}`);
  for (const [key, rows] of groups) {
    const [encoder, size] = key.split('|');
    const emitted = rows.filter((r) => r.match !== 'lost');
    const lost = rows.length - emitted.length;
    const lat = (a, b) => stats(emitted.filter((r) => r[a] !== null && r[b] !== null).map((r) => ms100(num(r[b]) - num(r[a]))));
    const capOut = lat('capture_qpc', 'pkt_out_qpc');
    const arrOut = lat('arrival_qpc', 'pkt_out_qpc');
    const inOut = lat('enc_in_qpc', 'pkt_out_qpc');
    const capArr = lat('capture_qpc', 'arrival_qpc');

    // 재정렬: 같은 세그먼트 안에서 출력 순서의 in_pts가 줄어들거나 같은 경우.
    let reorder = 0;
    let orderChecked = 0;
    let prev = null;
    for (const r of emitted) {
      const p = num(r.in_pts);
      if (p === null) continue;
      if (prev && prev.seg === r.seg) {
        orderChecked += 1;
        if (p <= prev.p) reorder += 1;
      }
      prev = { seg: r.seg, p };
    }
    // 입력 순서(캡처 QPC)와 출력 순서가 같은지도 따로 본다.
    let capReorder = 0;
    prev = null;
    for (const r of emitted) {
      const c = num(r.capture_qpc);
      if (c === null) continue;
      if (prev && prev.seg === r.seg && c < prev.p) capReorder += 1;
      prev = { seg: r.seg, p: c };
    }

    const withPts = emitted.filter((r) => r.pkt_pts !== null && r.capture_qpc !== null);
    const ptsEqCap = withPts.filter((r) => num(r.pkt_pts) === num(r.capture_qpc)).length;
    const ptsEqIn = emitted.filter((r) => r.pkt_pts !== null && r.in_pts !== null && num(r.pkt_pts) === num(r.in_pts)).length;
    const withDts = emitted.filter((r) => r.pkt_dts !== null && r.pkt_pts !== null);
    const dtsNe = withDts.filter((r) => num(r.pkt_dts) !== num(r.pkt_pts)).length;
    const noDts = emitted.filter((r) => r.pkt_dts === null).length;
    const noPts = emitted.filter((r) => r.pkt_pts === null).length;

    const matching = { exact: 0, skip: 0, fallback: 0 };
    for (const r of emitted) if (r.match in matching) matching[r.match] += 1;

    // 키프레임 간격(캡처 시각 기준 초, 같은 세그먼트 안)
    const keys = emitted.filter((r) => r.key_out === '1' && r.matched_qpc !== null);
    const gaps = [];
    for (let i = 1; i < keys.length; i++) {
      if (keys[i].seg === keys[i - 1].seg) gaps.push((num(keys[i].matched_qpc) - num(keys[i - 1].matched_qpc)) / QPC);
    }
    const keyReqNotOut = emitted.filter((r) => r.key_req === '1' && r.key_out !== '1').length;
    // 요청하지 않았는데 인코더가 스스로 낸 키프레임(장면 전환 감지 등)
    const keyUnreq = emitted.filter((r) => r.key_out === '1' && r.key_req !== '1').length;
    const maxPending = Math.max(0, ...rows.map((r) => num(r.pending) ?? 0));
    const bytes = stats(emitted.map((r) => num(r.bytes)));

    // 프레임 간격(캡처 시각, ms) — 틱이 30fps면 33.3
    const caps = emitted.map((r) => num(r.matched_qpc)).filter((x) => x !== null);
    const intervals = [];
    for (let i = 1; i < caps.length; i++) {
      const d = ms100(caps[i] - caps[i - 1]);
      if (d > 0 && d < 1000) intervals.push(d);
    }

    out.push({
      encoder, size,
      frames: emitted.length, lost,
      capOut, arrOut, inOut, capArr,
      reorder, orderChecked, capReorder,
      ptsEqCap, ptsEqCapN: withPts.length, ptsEqIn,
      dtsNe, dtsN: withDts.length, noDts, noPts,
      matching,
      keyframes: keys.length, keyGapSec: stats(gaps), keyReqNotOut, keyUnreq,
      maxPending, bytes, interval: stats(intervals),
    });
  }
  return out;
}

// ───────────────────────── 오디오 ─────────────────────────

/** audio.csv: 세그먼트(파이프라인 시작)마다 간격·지연을 본다. */
export function summarizeAudio(parsed) {
  if (!parsed || parsed.rows.length === 0) return null;
  const rows = parsed.rows;
  const dFirst = []; // first_in_qpc 간격(100ns)
  const dCap = [];
  const dIdx = [];
  const dEmit = [];
  let prev = null;
  for (const r of rows) {
    const cur = {
      seg: r.seg,
      idx: num(r.sample_index),
      first: num(r.first_in_qpc),
      cap: num(r.capture_qpc),
      emit: num(r.emit_qpc),
    };
    if (prev && prev.seg === cur.seg) {
      dFirst.push(cur.first - prev.first);
      dCap.push(cur.cap - prev.cap);
      dIdx.push(cur.idx - prev.idx);
      dEmit.push(ms100(cur.emit - prev.emit));
    }
    prev = cur;
  }
  const exact = (arr, v) => arr.filter((d) => d === v).length;
  const lag = stats(rows.map((r) => ms100(num(r.emit_qpc) - num(r.first_in_qpc))));
  const delays = [...new Set(rows.map((r) => r.delay_samples))].join('/');
  // 지연 반영 캡처 − 첫 입력 = -delay × 10^7/48000 (샘플 312 → -65000)
  const delayShift = stats(rows.map((r) => (num(r.capture_qpc) - num(r.first_in_qpc)) / 1e4));
  return {
    packets: rows.length,
    segments: new Set(rows.map((r) => r.seg)).size,
    pairs: dFirst.length,
    firstExact20: exact(dFirst, 200000),
    capExact20: exact(dCap, 200000),
    idxExact960: exact(dIdx, 960),
    firstGapMs: stats(dFirst.map(ms100)),
    emitGapMs: stats(dEmit),
    emitLagMs: lag,
    delaySamples: delays,
    delayShiftMs: delayShift,
    bytes: stats(rows.map((r) => num(r.bytes))),
  };
}

// ───────────────────────── 송출 ─────────────────────────

const RTP_RATE = { v: 90000, a: 48000 };

/** publish.csv: 종류별로 쓰기 결과와, RTP 증가가 캡처 시각 증가와 맞는지. */
export function summarizePublish(parsed) {
  if (!parsed || parsed.rows.length === 0) return [];
  const out = [];
  const groups = groupBy(parsed.rows, (r) => `${r.kind}|${r.pt ?? ""}`);
  for (const [key, rows] of groups) {
    const [kind, pt] = key.split('|');
    const rate = RTP_RATE[kind];
    const ok = rows.filter((r) => r.result === 'ok');
    const bad = rows.length - ok.length;
    const results = {};
    for (const r of rows) results[r.result] = (results[r.result] ?? 0) + 1;
    // 같은 세그먼트(= 한 송출 연결) 안에서 RTP 증가 − 캡처 증가(ms)
    const diffs = [];
    const offsets = []; // 캡처(µs) − RTP(µs): 일정해야 한다
    let prev = null;
    for (const r of ok) {
      const rtp = num(r.rtp);
      const cap = num(r.capture_epoch_us);
      if (rate && rtp !== null && cap !== null) {
        offsets.push({ seg: r.seg, v: cap - (rtp * 1e6) / rate });
        if (prev && prev.seg === r.seg) {
          diffs.push(((rtp - prev.rtp) * 1000) / rate - (cap - prev.cap) / 1000);
        }
        prev = { seg: r.seg, rtp, cap };
      }
    }
    // 오프셋 범위: 세그먼트마다 최대−최소
    const spreads = [];
    for (const [, segRows] of groupBy(offsets, (o) => o.seg)) {
      const vs = segRows.map((o) => o.v);
      spreads.push((Math.max(...vs) - Math.min(...vs)) / 1000);
    }
    const lag = stats(ok.map((r) => num(r.dequeue_lag_us) / 1000));
    out.push({
      kind, pt, n: rows.length, bad, results,
      diffAbs: stats(diffs.map(Math.abs)),
      diffMax: diffs.length ? Math.max(...diffs.map(Math.abs)) : NaN,
      offsetSpreadMs: spreads.length ? Math.max(...spreads) : NaN,
      lag,
    });
  }
  return out;
}

// ───────────────────────── 마크다운 ─────────────────────────

export function summarize({ video, audio, publish }) {
  const v = summarizeVideo(video);
  const a = summarizeAudio(audio);
  const p = summarizePublish(publish);
  const md = [];

  md.push('### 비디오: 캡처 → 인코더 출력 지연(ms)\n');
  md.push(
    table(
      ['인코더', '크기', '프레임', '캡처→출력 평균', 'p50', 'p95', '최대', '도착→출력 평균', '입력→출력 평균', '캡처→도착 평균'],
      v.map((x) => [
        x.encoder, x.size, x.frames,
        f1(x.capOut.avg), f1(x.capOut.p50), f1(x.capOut.p95), f1(x.capOut.max),
        f1(x.arrOut.avg), f1(x.inOut.avg), f1(x.capArr.avg),
      ]),
    ),
  );
  md.push('### 비디오: 시각 짝짓기와 순서\n');
  md.push(
    table(
      ['인코더', '크기', '출력 순서 어긋남', '캡처순 어긋남', 'pkt pts==캡처', 'pkt pts==입력 pts', 'dts≠pts', 'dts 없음', '짝: exact/skip/fallback', '출력 없는 입력(lost)', '대기열 최대', '프레임 간격 평균/최대 ms'],
      v.map((x) => [
        x.encoder, x.size,
        `${x.reorder}/${x.orderChecked}`, `${x.capReorder}`,
        pct(x.ptsEqCap, x.ptsEqCapN), pct(x.ptsEqIn, x.frames),
        x.dtsN > 0 ? pct(x.dtsNe, x.dtsN) : '-', x.noDts,
        `${x.matching.exact}/${x.matching.skip}/${x.matching.fallback}`,
        x.lost, x.maxPending,
        `${f1(x.interval.avg)}/${f1(x.interval.max)}`,
      ]),
    ),
  );
  md.push('### 비디오: 키프레임·크기\n');
  md.push(
    table(
      ['인코더', '크기', '키프레임 수', '간격 평균 s', '간격 최대 s', '요청했는데 키 아님', '요청 안 했는데 키', '패킷 바이트 평균', '최대'],
      v.map((x) => [
        x.encoder, x.size, x.keyframes, f2(x.keyGapSec.avg), f2(x.keyGapSec.max), x.keyReqNotOut, x.keyUnreq,
        Math.round(x.bytes.avg || 0), x.bytes.max || 0,
      ]),
    ),
  );

  md.push('### 오디오(Opus)\n');
  if (a) {
    md.push(
      table(
        ['패킷', '구간 수', '첫 샘플 QPC 간격 == 20.000ms', '지연 반영 QPC 간격 == 20.000ms', 'sample_index 증가 == 960', '내보냄 간격 평균/p95/최대 ms', '첫 샘플→내보냄 평균/p95/최대 ms', 'encoder_delay_samples', '캡처−첫 샘플 ms'],
        [[
          a.packets, a.segments,
          `${a.firstExact20}/${a.pairs}`, `${a.capExact20}/${a.pairs}`, `${a.idxExact960}/${a.pairs}`,
          `${f1(a.emitGapMs.avg)}/${f1(a.emitGapMs.p95)}/${f1(a.emitGapMs.max)}`,
          `${f1(a.emitLagMs.avg)}/${f1(a.emitLagMs.p95)}/${f1(a.emitLagMs.max)}`,
          a.delaySamples, f2(a.delayShiftMs.avg),
        ]],
      ),
    );
  } else md.push('(audio.csv 없음)\n');

  md.push('### 송출(publish.csv): 쓰기 결과와 RTP ↔ 캡처 시각\n');
  md.push(
    table(
      ['종류', 'PT', '쓰기', '실패', '결과', 'RTP증가−캡처증가 평균|절대| ms', '최대 ms', '캡처−RTP 오프셋 흔들림 ms', '큐에서 꺼냄−캡처 평균/p95/최대 ms'],
      p.map((x) => [
        x.kind === 'v' ? '비디오' : '오디오', x.pt, x.n, x.bad,
        Object.entries(x.results).map(([k, n]) => `${k}:${n}`).join(' '),
        f2(x.diffAbs.avg), f2(x.diffMax), f2(x.offsetSpreadMs),
        `${f1(x.lag.avg)}/${f1(x.lag.p95)}/${f1(x.lag.max)}`,
      ]),
    ),
  );
  return { markdown: md.join('\n'), data: { video: v, audio: a, publish: p } };
}

/** 폴더를 읽어 요약한다. */
export function summarizeDir(dir) {
  return summarize(readDir(dir));
}

// 직접 실행: node trace-summary.mjs <폴더>
if (process.argv[1] && path.resolve(process.argv[1]) === path.resolve(fileURLToPath(import.meta.url))) {
  const dir = process.argv[2];
  if (!dir) {
    console.error('사용법: node trace-summary.mjs <DONGSI_MEDIA_TRACE 폴더>');
    process.exit(2);
  }
  if (!fs.existsSync(dir)) {
    console.error(`폴더가 없습니다: ${dir}`);
    process.exit(2);
  }
  console.log(summarizeDir(dir).markdown);
}
