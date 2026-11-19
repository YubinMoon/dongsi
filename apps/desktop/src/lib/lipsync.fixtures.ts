// 립싱크 보정기 시험 자료: 실제 앱 실행에서 500ms 간격으로 읽은 통계 순서열(2026-11-20 팀장 실측 원자료, 값은 그대로).
// 그때는 옛 보정기가 바닥을 걸고 있었다. 그래서 `avOffsetMs`는 옛 보정기의 영향을 받은 값이고, 이 자료를 새 보정기에 넣는 것은
// "열린 고리 재생"이다(보정기가 바닥을 바꿔도 다음 값이 달라지지 않는다). "움직이지 않음"·"늘지 않음"만 확인하는 데 쓴다.
// 구간 평균 값(`audioJitterMs` 등)은 직전 원소와의 차이로 구한 값이다. `t`는 시청 첫 프레임 기준 ms.

export interface LipSyncFixtureRow {
  t: number;
  /** 비디오 EPT − 오디오 EPT(ms). 아직 없으면 null */
  avOffsetMs: number | null;
  framesDecoded: number;
  audioPackets: number;
  videoTargetMs: number | null;
  audioTargetMs: number | null;
  audioJitterMs: number | null;
  videoJitterMs: number | null;
  concealedSamples: number;
  freezeCount: number;
}

/** lan / h264: 4.0초에 오디오·비디오가 함께 끊김(은닉 +15000, 멈춤 +1). 오디오 목표 40 → 372~400ms(약 5초), 이어서 지연이 3.5초에 걸쳐 빠졌다. */
export const LIPSYNC_FIXTURE_LAN_H264: LipSyncFixtureRow[] = [
  { t: 8, avOffsetMs: null, framesDecoded: 2, audioPackets: 6, videoTargetMs: null, audioTargetMs: null, audioJitterMs: null, videoJitterMs: null, concealedSamples: 0, freezeCount: 0 },
  { t: 508, avOffsetMs: null, framesDecoded: 17, audioPackets: 31, videoTargetMs: 11, audioTargetMs: 80, audioJitterMs: 72, videoJitterMs: 6, concealedSamples: 0, freezeCount: 0 },
  { t: 1016, avOffsetMs: null, framesDecoded: 33, audioPackets: 56, videoTargetMs: 12, audioTargetMs: 43, audioJitterMs: 62, videoJitterMs: 5, concealedSamples: 0, freezeCount: 0 },
  { t: 1508, avOffsetMs: null, framesDecoded: 47, audioPackets: 81, videoTargetMs: 11, audioTargetMs: 40, audioJitterMs: 62, videoJitterMs: 7, concealedSamples: 0, freezeCount: 0 },
  { t: 2009, avOffsetMs: 13, framesDecoded: 59, audioPackets: 106, videoTargetMs: 19, audioTargetMs: 40, audioJitterMs: 64, videoJitterMs: 13, concealedSamples: 0, freezeCount: 0 },
  { t: 2508, avOffsetMs: 13, framesDecoded: 74, audioPackets: 131, videoTargetMs: 100, audioTargetMs: 40, audioJitterMs: 63, videoJitterMs: 96, concealedSamples: 0, freezeCount: 0 },
  { t: 3009, avOffsetMs: 13, framesDecoded: 89, audioPackets: 155, videoTargetMs: 100, audioTargetMs: 40, audioJitterMs: 63, videoJitterMs: 93, concealedSamples: 0, freezeCount: 0 },
  { t: 3509, avOffsetMs: 15, framesDecoded: 104, audioPackets: 181, videoTargetMs: 100, audioTargetMs: 40, audioJitterMs: 61, videoJitterMs: 94, concealedSamples: 0, freezeCount: 0 },
  { t: 4009, avOffsetMs: -41, framesDecoded: 117, audioPackets: 206, videoTargetMs: 100, audioTargetMs: 40, audioJitterMs: 54, videoJitterMs: 118, concealedSamples: 15000, freezeCount: 1 },
  { t: 4509, avOffsetMs: 273, framesDecoded: 132, audioPackets: 230, videoTargetMs: 96, audioTargetMs: 40, audioJitterMs: 255, videoJitterMs: 129, concealedSamples: 15000, freezeCount: 1 },
  { t: 5009, avOffsetMs: 279, framesDecoded: 147, audioPackets: 255, videoTargetMs: 111, audioTargetMs: 372, audioJitterMs: 379, videoJitterMs: 148, concealedSamples: 15000, freezeCount: 1 },
  { t: 5508, avOffsetMs: 288, framesDecoded: 163, audioPackets: 280, videoTargetMs: 113, audioTargetMs: 400, audioJitterMs: 383, videoJitterMs: 141, concealedSamples: 15000, freezeCount: 1 },
  { t: 6008, avOffsetMs: 290, framesDecoded: 178, audioPackets: 305, videoTargetMs: 115, audioTargetMs: 400, audioJitterMs: 381, videoJitterMs: 136, concealedSamples: 15000, freezeCount: 1 },
  { t: 6508, avOffsetMs: 295, framesDecoded: 193, audioPackets: 329, videoTargetMs: 119, audioTargetMs: 400, audioJitterMs: 383, videoJitterMs: 134, concealedSamples: 15000, freezeCount: 1 },
  { t: 7009, avOffsetMs: 105, framesDecoded: 202, audioPackets: 355, videoTargetMs: 313, audioTargetMs: 400, audioJitterMs: 383, videoJitterMs: 300, concealedSamples: 15000, freezeCount: 2 },
  { t: 7510, avOffsetMs: 109, framesDecoded: 217, audioPackets: 380, videoTargetMs: 314, audioTargetMs: 400, audioJitterMs: 383, videoJitterMs: 322, concealedSamples: 15000, freezeCount: 2 },
  { t: 8010, avOffsetMs: 110, framesDecoded: 232, audioPackets: 405, videoTargetMs: 314, audioTargetMs: 400, audioJitterMs: 381, videoJitterMs: 318, concealedSamples: 15000, freezeCount: 2 },
  { t: 8509, avOffsetMs: 112, framesDecoded: 247, audioPackets: 430, videoTargetMs: 314, audioTargetMs: 400, audioJitterMs: 384, videoJitterMs: 315, concealedSamples: 15000, freezeCount: 2 },
  { t: 9009, avOffsetMs: 115, framesDecoded: 263, audioPackets: 456, videoTargetMs: 314, audioTargetMs: 400, audioJitterMs: 383, videoJitterMs: 310, concealedSamples: 15000, freezeCount: 2 },
  { t: 9508, avOffsetMs: 115, framesDecoded: 278, audioPackets: 481, videoTargetMs: 314, audioTargetMs: 400, audioJitterMs: 384, videoJitterMs: 311, concealedSamples: 15000, freezeCount: 2 },
  { t: 10008, avOffsetMs: 23, framesDecoded: 293, audioPackets: 506, videoTargetMs: 314, audioTargetMs: 139, audioJitterMs: 329, videoJitterMs: 309, concealedSamples: 15000, freezeCount: 2 },
  { t: 10508, avOffsetMs: -109, framesDecoded: 304, audioPackets: 531, videoTargetMs: 429, audioTargetMs: 40, audioJitterMs: 279, videoJitterMs: 415, concealedSamples: 15000, freezeCount: 2 },
  { t: 11009, avOffsetMs: -147, framesDecoded: 319, audioPackets: 556, videoTargetMs: 429, audioTargetMs: 40, audioJitterMs: 240, videoJitterMs: 424, concealedSamples: 15000, freezeCount: 2 },
  { t: 11508, avOffsetMs: -172, framesDecoded: 334, audioPackets: 581, videoTargetMs: 429, audioTargetMs: 40, audioJitterMs: 220, videoJitterMs: 423, concealedSamples: 15000, freezeCount: 2 },
  { t: 12008, avOffsetMs: -210, framesDecoded: 350, audioPackets: 606, videoTargetMs: 429, audioTargetMs: 40, audioJitterMs: 176, videoJitterMs: 421, concealedSamples: 15000, freezeCount: 2 },
  { t: 12510, avOffsetMs: -37, framesDecoded: 371, audioPackets: 631, videoTargetMs: 229, audioTargetMs: 40, audioJitterMs: 158, videoJitterMs: 258, concealedSamples: 15000, freezeCount: 2 },
  { t: 13008, avOffsetMs: -64, framesDecoded: 386, audioPackets: 656, videoTargetMs: 229, audioTargetMs: 40, audioJitterMs: 121, videoJitterMs: 220, concealedSamples: 15000, freezeCount: 2 },
  { t: 13508, avOffsetMs: -104, framesDecoded: 401, audioPackets: 681, videoTargetMs: 229, audioTargetMs: 40, audioJitterMs: 97, videoJitterMs: 223, concealedSamples: 15000, freezeCount: 2 },
  { t: 14008, avOffsetMs: -116, framesDecoded: 416, audioPackets: 706, videoTargetMs: 229, audioTargetMs: 40, audioJitterMs: 63, videoJitterMs: 221, concealedSamples: 15000, freezeCount: 2 },
  { t: 14508, avOffsetMs: -116, framesDecoded: 431, audioPackets: 731, videoTargetMs: 229, audioTargetMs: 40, audioJitterMs: 63, videoJitterMs: 221, concealedSamples: 15000, freezeCount: 2 },
  { t: 15008, avOffsetMs: -24, framesDecoded: 448, audioPackets: 756, videoTargetMs: 137, audioTargetMs: 40, audioJitterMs: 63, videoJitterMs: 157, concealedSamples: 15000, freezeCount: 2 },
  { t: 15523, avOffsetMs: -23, framesDecoded: 464, audioPackets: 781, videoTargetMs: 137, audioTargetMs: 40, audioJitterMs: 62, videoJitterMs: 127, concealedSamples: 15000, freezeCount: 2 },
  { t: 16008, avOffsetMs: -24, framesDecoded: 478, audioPackets: 806, videoTargetMs: 137, audioTargetMs: 40, audioJitterMs: 65, videoJitterMs: 128, concealedSamples: 15000, freezeCount: 2 },
  { t: 16508, avOffsetMs: -24, framesDecoded: 493, audioPackets: 831, videoTargetMs: 137, audioTargetMs: 40, audioJitterMs: 64, videoJitterMs: 127, concealedSamples: 15000, freezeCount: 2 },
  { t: 17009, avOffsetMs: -24, framesDecoded: 508, audioPackets: 856, videoTargetMs: 137, audioTargetMs: 40, audioJitterMs: 63, videoJitterMs: 128, concealedSamples: 15000, freezeCount: 2 },
  { t: 17508, avOffsetMs: -24, framesDecoded: 523, audioPackets: 881, videoTargetMs: 137, audioTargetMs: 40, audioJitterMs: 62, videoJitterMs: 125, concealedSamples: 15000, freezeCount: 2 },
  { t: 18009, avOffsetMs: -24, framesDecoded: 538, audioPackets: 906, videoTargetMs: 137, audioTargetMs: 40, audioJitterMs: 64, videoJitterMs: 127, concealedSamples: 15000, freezeCount: 2 },
  { t: 18508, avOffsetMs: -24, framesDecoded: 553, audioPackets: 931, videoTargetMs: 137, audioTargetMs: 40, audioJitterMs: 63, videoJitterMs: 127, concealedSamples: 15000, freezeCount: 2 },
  { t: 19008, avOffsetMs: -24, framesDecoded: 568, audioPackets: 956, videoTargetMs: 137, audioTargetMs: 40, audioJitterMs: 64, videoJitterMs: 129, concealedSamples: 15000, freezeCount: 2 },
  { t: 19508, avOffsetMs: -23, framesDecoded: 583, audioPackets: 981, videoTargetMs: 137, audioTargetMs: 40, audioJitterMs: 61, videoJitterMs: 128, concealedSamples: 15000, freezeCount: 2 },
  { t: 20008, avOffsetMs: -23, framesDecoded: 598, audioPackets: 1006, videoTargetMs: 137, audioTargetMs: 40, audioJitterMs: 63, videoJitterMs: 128, concealedSamples: 15000, freezeCount: 2 },
  { t: 20509, avOffsetMs: -23, framesDecoded: 613, audioPackets: 1031, videoTargetMs: 137, audioTargetMs: 40, audioJitterMs: 62, videoJitterMs: 128, concealedSamples: 15000, freezeCount: 2 },
  { t: 21008, avOffsetMs: -23, framesDecoded: 628, audioPackets: 1056, videoTargetMs: 137, audioTargetMs: 40, audioJitterMs: 63, videoJitterMs: 126, concealedSamples: 15000, freezeCount: 2 },
  { t: 21508, avOffsetMs: -22, framesDecoded: 643, audioPackets: 1081, videoTargetMs: 137, audioTargetMs: 40, audioJitterMs: 63, videoJitterMs: 126, concealedSamples: 15000, freezeCount: 2 },
  { t: 22009, avOffsetMs: -23, framesDecoded: 658, audioPackets: 1106, videoTargetMs: 137, audioTargetMs: 40, audioJitterMs: 64, videoJitterMs: 128, concealedSamples: 15000, freezeCount: 2 },
  { t: 22509, avOffsetMs: -23, framesDecoded: 673, audioPackets: 1131, videoTargetMs: 137, audioTargetMs: 40, audioJitterMs: 61, videoJitterMs: 127, concealedSamples: 15000, freezeCount: 2 },
  { t: 23009, avOffsetMs: -23, framesDecoded: 688, audioPackets: 1156, videoTargetMs: 137, audioTargetMs: 40, audioJitterMs: 64, videoJitterMs: 126, concealedSamples: 15000, freezeCount: 2 },
  { t: 23509, avOffsetMs: -23, framesDecoded: 703, audioPackets: 1181, videoTargetMs: 137, audioTargetMs: 40, audioJitterMs: 63, videoJitterMs: 128, concealedSamples: 15000, freezeCount: 2 },
  { t: 24009, avOffsetMs: -23, framesDecoded: 718, audioPackets: 1206, videoTargetMs: 137, audioTargetMs: 40, audioJitterMs: 62, videoJitterMs: 130, concealedSamples: 15000, freezeCount: 2 },
  { t: 24508, avOffsetMs: -23, framesDecoded: 733, audioPackets: 1231, videoTargetMs: 137, audioTargetMs: 40, audioJitterMs: 62, videoJitterMs: 129, concealedSamples: 15000, freezeCount: 2 },
  { t: 25009, avOffsetMs: -23, framesDecoded: 748, audioPackets: 1256, videoTargetMs: 137, audioTargetMs: 40, audioJitterMs: 64, videoJitterMs: 126, concealedSamples: 15000, freezeCount: 2 },
];

/** server / vp8: 시작 은닉 +1440은 정상. 20.0초에 끊김(은닉 +7440, 멈춤 +1). 오디오 목표 40 → 149 → 185 → 100. */
export const LIPSYNC_FIXTURE_SERVER_VP8: LipSyncFixtureRow[] = [
  { t: 5, avOffsetMs: null, framesDecoded: 1, audioPackets: 2, videoTargetMs: null, audioTargetMs: null, audioJitterMs: null, videoJitterMs: null, concealedSamples: 480, freezeCount: 0 },
  { t: 507, avOffsetMs: null, framesDecoded: 16, audioPackets: 27, videoTargetMs: 12, audioTargetMs: 80, audioJitterMs: 41, videoJitterMs: 8, concealedSamples: 1920, freezeCount: 0 },
  { t: 1010, avOffsetMs: null, framesDecoded: 31, audioPackets: 51, videoTargetMs: 12, audioTargetMs: 43, audioJitterMs: 58, videoJitterMs: 7, concealedSamples: 1920, freezeCount: 0 },
  { t: 1506, avOffsetMs: null, framesDecoded: 46, audioPackets: 77, videoTargetMs: 12, audioTargetMs: 40, audioJitterMs: 58, videoJitterMs: 7, concealedSamples: 1920, freezeCount: 0 },
  { t: 2006, avOffsetMs: 93, framesDecoded: 58, audioPackets: 102, videoTargetMs: 13, audioTargetMs: 40, audioJitterMs: 54, videoJitterMs: 6, concealedSamples: 1920, freezeCount: 0 },
  { t: 2506, avOffsetMs: 12, framesDecoded: 73, audioPackets: 127, videoTargetMs: 95, audioTargetMs: 72, audioJitterMs: 58, videoJitterMs: 90, concealedSamples: 1920, freezeCount: 0 },
  { t: 3006, avOffsetMs: 12, framesDecoded: 88, audioPackets: 151, videoTargetMs: 95, audioTargetMs: 80, audioJitterMs: 59, videoJitterMs: 88, concealedSamples: 1920, freezeCount: 0 },
  { t: 3506, avOffsetMs: 13, framesDecoded: 103, audioPackets: 176, videoTargetMs: 95, audioTargetMs: 80, audioJitterMs: 56, videoJitterMs: 88, concealedSamples: 1920, freezeCount: 0 },
  { t: 4006, avOffsetMs: 14, framesDecoded: 118, audioPackets: 202, videoTargetMs: 94, audioTargetMs: 80, audioJitterMs: 54, videoJitterMs: 88, concealedSamples: 1920, freezeCount: 0 },
  { t: 4508, avOffsetMs: 1, framesDecoded: 133, audioPackets: 226, videoTargetMs: 96, audioTargetMs: 80, audioJitterMs: 54, videoJitterMs: 88, concealedSamples: 1920, freezeCount: 0 },
  { t: 5006, avOffsetMs: 0, framesDecoded: 148, audioPackets: 252, videoTargetMs: 108, audioTargetMs: 80, audioJitterMs: 55, videoJitterMs: 98, concealedSamples: 1920, freezeCount: 0 },
  { t: 5506, avOffsetMs: 0, framesDecoded: 163, audioPackets: 277, videoTargetMs: 108, audioTargetMs: 80, audioJitterMs: 54, videoJitterMs: 100, concealedSamples: 1920, freezeCount: 0 },
  { t: 6005, avOffsetMs: -1, framesDecoded: 178, audioPackets: 301, videoTargetMs: 108, audioTargetMs: 80, audioJitterMs: 55, videoJitterMs: 97, concealedSamples: 1920, freezeCount: 0 },
  { t: 6507, avOffsetMs: -1, framesDecoded: 193, audioPackets: 327, videoTargetMs: 108, audioTargetMs: 59, audioJitterMs: 56, videoJitterMs: 99, concealedSamples: 1920, freezeCount: 0 },
  { t: 7006, avOffsetMs: -2, framesDecoded: 208, audioPackets: 351, videoTargetMs: 108, audioTargetMs: 40, audioJitterMs: 59, videoJitterMs: 98, concealedSamples: 1920, freezeCount: 0 },
  { t: 7506, avOffsetMs: -3, framesDecoded: 223, audioPackets: 377, videoTargetMs: 108, audioTargetMs: 40, audioJitterMs: 54, videoJitterMs: 99, concealedSamples: 1920, freezeCount: 0 },
  { t: 8006, avOffsetMs: -3, framesDecoded: 238, audioPackets: 401, videoTargetMs: 108, audioTargetMs: 48, audioJitterMs: 58, videoJitterMs: 100, concealedSamples: 1920, freezeCount: 0 },
  { t: 8506, avOffsetMs: -2, framesDecoded: 253, audioPackets: 426, videoTargetMs: 108, audioTargetMs: 60, audioJitterMs: 58, videoJitterMs: 101, concealedSamples: 1920, freezeCount: 0 },
  { t: 9014, avOffsetMs: -2, framesDecoded: 268, audioPackets: 452, videoTargetMs: 108, audioTargetMs: 60, audioJitterMs: 57, videoJitterMs: 104, concealedSamples: 1920, freezeCount: 0 },
  { t: 9506, avOffsetMs: -2, framesDecoded: 282, audioPackets: 474, videoTargetMs: 108, audioTargetMs: 60, audioJitterMs: 58, videoJitterMs: 101, concealedSamples: 1920, freezeCount: 0 },
  { t: 10007, avOffsetMs: -2, framesDecoded: 297, audioPackets: 499, videoTargetMs: 108, audioTargetMs: 60, audioJitterMs: 59, videoJitterMs: 102, concealedSamples: 1920, freezeCount: 0 },
  { t: 10506, avOffsetMs: -1, framesDecoded: 312, audioPackets: 524, videoTargetMs: 108, audioTargetMs: 60, audioJitterMs: 57, videoJitterMs: 105, concealedSamples: 1920, freezeCount: 0 },
  { t: 11005, avOffsetMs: -1, framesDecoded: 327, audioPackets: 550, videoTargetMs: 108, audioTargetMs: 60, audioJitterMs: 57, videoJitterMs: 101, concealedSamples: 1920, freezeCount: 0 },
  { t: 11506, avOffsetMs: -1, framesDecoded: 342, audioPackets: 575, videoTargetMs: 108, audioTargetMs: 60, audioJitterMs: 54, videoJitterMs: 99, concealedSamples: 1920, freezeCount: 0 },
  { t: 12006, avOffsetMs: -1, framesDecoded: 358, audioPackets: 601, videoTargetMs: 108, audioTargetMs: 60, audioJitterMs: 58, videoJitterMs: 97, concealedSamples: 1920, freezeCount: 0 },
  { t: 12506, avOffsetMs: -1, framesDecoded: 373, audioPackets: 626, videoTargetMs: 108, audioTargetMs: 60, audioJitterMs: 56, videoJitterMs: 99, concealedSamples: 1920, freezeCount: 0 },
  { t: 13006, avOffsetMs: -1, framesDecoded: 388, audioPackets: 651, videoTargetMs: 108, audioTargetMs: 60, audioJitterMs: 59, videoJitterMs: 98, concealedSamples: 1920, freezeCount: 0 },
  { t: 13506, avOffsetMs: -2, framesDecoded: 403, audioPackets: 676, videoTargetMs: 108, audioTargetMs: 60, audioJitterMs: 60, videoJitterMs: 99, concealedSamples: 1920, freezeCount: 0 },
  { t: 14006, avOffsetMs: -2, framesDecoded: 417, audioPackets: 701, videoTargetMs: 108, audioTargetMs: 60, audioJitterMs: 56, videoJitterMs: 99, concealedSamples: 1920, freezeCount: 0 },
  { t: 14507, avOffsetMs: -1, framesDecoded: 433, audioPackets: 726, videoTargetMs: 108, audioTargetMs: 43, audioJitterMs: 58, videoJitterMs: 102, concealedSamples: 1920, freezeCount: 0 },
  { t: 15006, avOffsetMs: -1, framesDecoded: 448, audioPackets: 752, videoTargetMs: 108, audioTargetMs: 40, audioJitterMs: 58, videoJitterMs: 103, concealedSamples: 1920, freezeCount: 0 },
  { t: 15505, avOffsetMs: 0, framesDecoded: 463, audioPackets: 777, videoTargetMs: 108, audioTargetMs: 40, audioJitterMs: 57, videoJitterMs: 104, concealedSamples: 1920, freezeCount: 0 },
  { t: 16016, avOffsetMs: -1, framesDecoded: 478, audioPackets: 802, videoTargetMs: 108, audioTargetMs: 40, audioJitterMs: 56, videoJitterMs: 100, concealedSamples: 1920, freezeCount: 0 },
  { t: 16506, avOffsetMs: -1, framesDecoded: 493, audioPackets: 826, videoTargetMs: 108, audioTargetMs: 40, audioJitterMs: 56, videoJitterMs: 99, concealedSamples: 1920, freezeCount: 0 },
  { t: 17006, avOffsetMs: -1, framesDecoded: 508, audioPackets: 852, videoTargetMs: 108, audioTargetMs: 40, audioJitterMs: 59, videoJitterMs: 99, concealedSamples: 1920, freezeCount: 0 },
  { t: 17505, avOffsetMs: -3, framesDecoded: 523, audioPackets: 877, videoTargetMs: 108, audioTargetMs: 40, audioJitterMs: 57, videoJitterMs: 96, concealedSamples: 1920, freezeCount: 0 },
  { t: 18021, avOffsetMs: -2, framesDecoded: 538, audioPackets: 902, videoTargetMs: 108, audioTargetMs: 40, audioJitterMs: 58, videoJitterMs: 97, concealedSamples: 1920, freezeCount: 0 },
  { t: 18506, avOffsetMs: -3, framesDecoded: 552, audioPackets: 927, videoTargetMs: 108, audioTargetMs: 40, audioJitterMs: 58, videoJitterMs: 99, concealedSamples: 1920, freezeCount: 0 },
  { t: 19005, avOffsetMs: -2, framesDecoded: 568, audioPackets: 952, videoTargetMs: 108, audioTargetMs: 40, audioJitterMs: 57, videoJitterMs: 100, concealedSamples: 1920, freezeCount: 0 },
  { t: 19506, avOffsetMs: -4, framesDecoded: 582, audioPackets: 977, videoTargetMs: 108, audioTargetMs: 40, audioJitterMs: 58, videoJitterMs: 103, concealedSamples: 1920, freezeCount: 0 },
  { t: 20011, avOffsetMs: -5, framesDecoded: 595, audioPackets: 997, videoTargetMs: 108, audioTargetMs: 40, audioJitterMs: 58, videoJitterMs: 102, concealedSamples: 9360, freezeCount: 0 },
  { t: 20506, avOffsetMs: 65, framesDecoded: 607, audioPackets: 1025, videoTargetMs: 108, audioTargetMs: 149, audioJitterMs: 163, videoJitterMs: 107, concealedSamples: 9360, freezeCount: 1 },
  { t: 21006, avOffsetMs: 158, framesDecoded: 624, audioPackets: 1052, videoTargetMs: 108, audioTargetMs: 185, audioJitterMs: 199, videoJitterMs: 96, concealedSamples: 9360, freezeCount: 1 },
  { t: 21505, avOffsetMs: 157, framesDecoded: 639, audioPackets: 1077, videoTargetMs: 108, audioTargetMs: 100, audioJitterMs: 218, videoJitterMs: 98, concealedSamples: 9360, freezeCount: 1 },
  { t: 22005, avOffsetMs: 156, framesDecoded: 654, audioPackets: 1102, videoTargetMs: 108, audioTargetMs: 100, audioJitterMs: 219, videoJitterMs: 101, concealedSamples: 9360, freezeCount: 1 },
  { t: 22506, avOffsetMs: -13, framesDecoded: 664, audioPackets: 1127, videoTargetMs: 216, audioTargetMs: 100, audioJitterMs: 207, videoJitterMs: 190, concealedSamples: 9360, freezeCount: 2 },
  { t: 23006, avOffsetMs: -65, framesDecoded: 679, audioPackets: 1152, videoTargetMs: 262, audioTargetMs: 100, audioJitterMs: 173, videoJitterMs: 254, concealedSamples: 9360, freezeCount: 2 },
  { t: 23508, avOffsetMs: -94, framesDecoded: 694, audioPackets: 1177, videoTargetMs: 262, audioTargetMs: 100, audioJitterMs: 138, videoJitterMs: 255, concealedSamples: 9360, freezeCount: 2 },
  { t: 24009, avOffsetMs: -106, framesDecoded: 709, audioPackets: 1202, videoTargetMs: 262, audioTargetMs: 100, audioJitterMs: 120, videoJitterMs: 256, concealedSamples: 9360, freezeCount: 2 },
  { t: 24506, avOffsetMs: -106, framesDecoded: 724, audioPackets: 1227, videoTargetMs: 262, audioTargetMs: 100, audioJitterMs: 117, videoJitterMs: 258, concealedSamples: 9360, freezeCount: 2 },
  { t: 25005, avOffsetMs: -106, framesDecoded: 739, audioPackets: 1252, videoTargetMs: 262, audioTargetMs: 100, audioJitterMs: 116, videoJitterMs: 258, concealedSamples: 9360, freezeCount: 2 },
];
