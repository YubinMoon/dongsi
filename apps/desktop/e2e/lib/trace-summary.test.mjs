// trace-summary.mjs 시험: 작은 고정 CSV로 계산을 확인한다. 실행: node --test e2e/lib/trace-summary.test.mjs (apps/desktop에서)
import assert from 'node:assert/strict';
import test from 'node:test';

import { parseCsv, summarize, summarizeAudio, summarizePublish, summarizeVideo } from './trace-summary.mjs';

const VIDEO_HEADER =
  'encoder,w,h,capture_qpc,arrival_qpc,enc_in_qpc,pkt_out_qpc,in_pts,pkt_pts,pkt_dts,matched_qpc,match,key_req,key_pkt,key_out,bytes,pending';

// 프레임 33.3ms 간격(333333 단위). 캡처→출력: 10ms, 12ms, 14ms(= 100000, 120000, 140000).
const VIDEO = [
  VIDEO_HEADER,
  '# pipeline source=test qpc=1 epoch_us=2',
  'h264_nvenc,1280,720,1000000,1000100,1000200,1100000,1000000,1000000,1000000,1000000,exact,1,1,1,5000,0',
  'h264_nvenc,1280,720,1333333,1333400,1333500,1453333,1333333,1333333,1333000,1333333,exact,0,0,0,900,0',
  // 한 장은 출력 없이 버려짐(lost), 다음 장은 앞쪽을 건너뛰고 맞춤(skip)
  'h264_nvenc,1280,720,1666666,1666700,1666800,,1666666,,,,lost,0,0,0,0,1',
  'h264_nvenc,1280,720,2000000,2000100,2000200,2140000,2000000,2000000,2000000,2000000,skip,0,0,0,950,0',
  // 구분 줄 뒤 새 세그먼트: pts가 앞 세그먼트보다 작아도 재정렬이 아니다
  '# encoder name=h264_nvenc size=1280x720 fps=30 kbps=6000',
  'h264_nvenc,1280,720,500000,500050,500100,600000,500000,500000,500000,500000,exact,1,1,1,5000,0',
  // 크기가 다르면 따로 묶는다
  'h264_nvenc,1920,1080,1000000,1000100,1000200,1300000,1000000,1000000,1000000,1000000,exact,1,1,1,4000,0',
  'h264_nvenc,1920,1080,1333333,1333400,1333500,1633333,1333333,1333333,1333333,1333333,exact,0,0,0,800,0',
].join('\n');

test('parseCsv: 구분 줄은 세그먼트를 나누고 빈 칸은 null', () => {
  const { rows, marks } = parseCsv(VIDEO);
  assert.equal(rows.length, 7);
  assert.equal(marks.length, 2);
  assert.equal(marks[0].fields.label, 'pipeline');
  assert.equal(marks[0].fields.epoch_us, '2');
  assert.equal(marks[1].fields.label, 'encoder');
  assert.equal(marks[1].fields.name, 'h264_nvenc');
  assert.equal(marks[1].fields.size, '1280x720');
  assert.equal(marks[1].fields.kbps, '6000');
  assert.deepEqual(rows.map((r) => r.seg), [1, 1, 1, 1, 2, 2, 2]);
  assert.equal(rows[2].pkt_pts, null);
  assert.equal(rows[0].match, 'exact');
});

test('summarizeVideo: 지연·짝짓기·dts·키프레임', () => {
  const s = summarizeVideo(parseCsv(VIDEO));
  assert.equal(s.length, 2);
  const h = s.find((x) => x.size === '1280x720');
  assert.equal(h.encoder, 'h264_nvenc');
  assert.equal(h.frames, 4);
  assert.equal(h.lost, 1);
  // 캡처→출력: 10, 12, 14, 10 ms
  assert.equal(h.capOut.n, 4);
  assert.ok(Math.abs(h.capOut.avg - 11.5) < 1e-9, h.capOut.avg);
  assert.equal(h.capOut.max, 14);
  // 재정렬 없음(세그먼트 경계는 비교하지 않음)
  assert.equal(h.reorder, 0);
  assert.equal(h.orderChecked, 2);
  // pts == 캡처 QPC: 전부(이 자료는 테스트 패턴처럼 캡처 시각이 곧 격자 칸 시각이다)
  assert.equal(h.ptsEqCap, 4);
  // dts ≠ pts: 한 줄(두 번째)
  assert.equal(h.dtsNe, 1);
  assert.equal(h.dtsN, 4);
  assert.deepEqual(h.matching, { exact: 3, skip: 1, fallback: 0 });
  // 키프레임 2개, 서로 다른 세그먼트라 간격은 없다
  assert.equal(h.keyframes, 2);
  assert.equal(h.keyUnreq, 0);
  assert.equal(h.keyGapSec.n, 0);
  assert.equal(h.maxPending, 1);
  const v = s.find((x) => x.size === '1920x1080');
  assert.ok(Math.abs(v.capOut.avg - 30) < 1e-9);
});

test('summarizeVideo: 출력 순서가 뒤바뀌면 센다', () => {
  const text = [
    VIDEO_HEADER,
    'x,64,64,100,100,100,200,100,100,100,100,exact,0,0,0,1,0',
    'x,64,64,300,300,300,400,300,300,300,300,exact,0,0,0,1,0',
    'x,64,64,200,200,200,500,200,200,,200,exact,0,0,0,1,0',
  ].join('\n');
  const [x] = summarizeVideo(parseCsv(text));
  assert.equal(x.reorder, 1);
  assert.equal(x.capReorder, 1);
  assert.equal(x.noDts, 1);
});

test('summarizeVideo: 키프레임 간격(초)', () => {
  const rows = [VIDEO_HEADER];
  for (let i = 0; i < 4; i++) {
    const q = 1e7 * i * 2; // 2초마다
    rows.push(`h264_nvenc,64,64,${q},${q},${q},${q + 1000},${q},${q},${q},${q},exact,1,1,1,10,0`);
  }
  const [x] = summarizeVideo(parseCsv(rows.join('\n')));
  assert.equal(x.keyframes, 4);
  assert.equal(x.keyGapSec.avg, 2);
  assert.equal(x.keyUnreq, 0);
});

const AUDIO = [
  'sample_index,first_in_qpc,capture_qpc,emit_qpc,delay_samples,bytes',
  '# pipeline source=TestTone base_qpc=0 delay_samples=312',
  '0,0,-65000,150000,312,80',
  '960,200000,135000,350000,312,80',
  '1920,400000,335000,600000,312,80',
  '3840,800000,735000,1000000,312,80', // 한 패킷이 빠졌다(960×2 증가)
].join('\n');

test('summarizeAudio: 20ms 간격과 지연', () => {
  const a = summarizeAudio(parseCsv(AUDIO));
  assert.equal(a.packets, 4);
  assert.equal(a.pairs, 3);
  assert.equal(a.firstExact20, 2);
  assert.equal(a.capExact20, 2);
  assert.equal(a.idxExact960, 2);
  assert.equal(a.delaySamples, '312');
  assert.ok(Math.abs(a.delayShiftMs.avg - -6.5) < 1e-9);
  assert.ok(Math.abs(a.emitLagMs.avg - ((15 + 15 + 20 + 20) / 4)) < 1e-9);
});

const PUBLISH = [
  'kind,pt,rtp,capture_epoch_us,dequeue_lag_us,result',
  '# publisher epoch_us=1000',
  'v,102,900000,10000000,500,ok',
  'v,102,903000,10033333,600,ok', // 3000/90000 = 33.333ms
  'v,102,906000,10066667,700,ok',
  'v,102,909000,10100000,800,err:boom',
  'a,111,0,10000000,100,ok',
  'a,111,960,10020000,100,ok',
  'a,111,1920,10040500,100,ok', // 500µs 어긋남
  'a,,2880,10060000,100,no_pt',
].join('\n');

test('summarizePublish: RTP 증가와 캡처 시각 증가', () => {
  const p = summarizePublish(parseCsv(PUBLISH));
  const v = p.find((x) => x.kind === 'v');
  assert.equal(v.n, 4);
  assert.equal(v.bad, 1);
  assert.equal(v.results.ok, 3);
  assert.ok(v.diffMax < 0.01, v.diffMax);
  const a = p.find((x) => x.kind === 'a' && x.pt === '111');
  assert.equal(a.n, 3);
  assert.ok(Math.abs(a.diffMax - 0.5) < 1e-6, a.diffMax);
  assert.ok(a.offsetSpreadMs > 0.4 && a.offsetSpreadMs < 0.6, a.offsetSpreadMs);
  const miss = p.find((x) => x.kind === 'a' && x.pt === '');
  assert.equal(miss.bad, 1);
});

test('summarize: 마크다운 표가 나온다', () => {
  const { markdown, data } = summarize({
    video: parseCsv(VIDEO),
    audio: parseCsv(AUDIO),
    publish: parseCsv(PUBLISH),
  });
  assert.match(markdown, /### 비디오: 캡처 → 인코더 출력 지연/);
  assert.match(markdown, /\| h264_nvenc \| 1280x720 \| 4 \|/);
  assert.match(markdown, /### 오디오\(Opus\)/);
  assert.match(markdown, /### 송출\(publish\.csv\)/);
  assert.equal(data.video.length, 2);
  // 없는 파일은 자료 없음으로 나온다
  const empty = summarize({ video: null, audio: null, publish: null });
  assert.match(empty.markdown, /자료 없음/);
});
