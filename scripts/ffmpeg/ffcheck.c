// 결과 DLL을 확인하는 작은 프로그램(결과물 bin/에는 넣지 않는다).
// avcodec_configuration(), 세 라이브러리의 *_license(), 인코더·디코더 존재를 출력한다.
// 기대와 다르면 종료 코드 1.
#include <stdio.h>
#include <string.h>
#include <libavcodec/avcodec.h>
#include <libavutil/avutil.h>
#include <libswscale/swscale.h>

static const char *const WANT_ENC[] = {"h264_nvenc", "h264_amf", "h264_qsv", "libopus", NULL};
static const char *const WANT_DEC[] = {"h264", "libopus", NULL};
static const char *const WANT_LICENSE = "LGPL version 2.1 or later";

int main(void) {
    int bad = 0;
    printf("avcodec_version: %u.%u.%u\n", AV_VERSION_MAJOR(avcodec_version()),
           AV_VERSION_MINOR(avcodec_version()), AV_VERSION_MICRO(avcodec_version()));
    printf("avutil_version: %u.%u.%u\n", AV_VERSION_MAJOR(avutil_version()),
           AV_VERSION_MINOR(avutil_version()), AV_VERSION_MICRO(avutil_version()));
    printf("swscale_version: %u.%u.%u\n", AV_VERSION_MAJOR(swscale_version()),
           AV_VERSION_MINOR(swscale_version()), AV_VERSION_MICRO(swscale_version()));
    printf("avcodec_configuration: %s\n", avcodec_configuration());
    printf("avcodec_license: %s\n", avcodec_license());
    printf("avutil_license: %s\n", avutil_license());
    printf("swscale_license: %s\n", swscale_license());
    if (strcmp(avcodec_license(), WANT_LICENSE) || strcmp(avutil_license(), WANT_LICENSE) ||
        strcmp(swscale_license(), WANT_LICENSE)) {
        printf("FAIL: 라이선스가 '%s'가 아니다\n", WANT_LICENSE);
        bad = 1;
    }
    for (int i = 0; WANT_ENC[i]; i++) {
        int ok = avcodec_find_encoder_by_name(WANT_ENC[i]) != NULL;
        printf("encoder %s: %s\n", WANT_ENC[i], ok ? "있음" : "없음");
        bad |= !ok;
    }
    for (int i = 0; WANT_DEC[i]; i++) {
        int ok = avcodec_find_decoder_by_name(WANT_DEC[i]) != NULL;
        printf("decoder %s: %s\n", WANT_DEC[i], ok ? "있음" : "없음");
        bad |= !ok;
    }
    // 등록된 인코더·디코더 전체(구성 요소가 더 들어가지 않았는지 보려고)
    void *it = NULL;
    const AVCodec *c;
    printf("registered encoders:");
    while ((c = av_codec_iterate(&it)))
        if (av_codec_is_encoder(c)) printf(" %s", c->name);
    printf("\nregistered decoders:");
    it = NULL;
    while ((c = av_codec_iterate(&it)))
        if (av_codec_is_decoder(c)) printf(" %s", c->name);
    printf("\n");
    printf(bad ? "RESULT: FAIL\n" : "RESULT: OK\n");
    return bad;
}
