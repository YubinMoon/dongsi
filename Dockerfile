# syntax=docker/dockerfile:1
#
# dongsi 전용 서버(dongsi-server) 이미지. 저장소 루트에서 빌드한다.
#
#   docker build -t dongsi-server:latest .
#
# 사용법과 설정은 deploy/README.md를 본다.

# ---- 빌드 ----
FROM rust:1-slim-trixie AS build

# 추가 패키지는 필요 없다. aws-lc-sys(rustls의 암호 라이브러리)는 이 이미지에 들어 있는 gcc로 빌드된다
# (cmake·clang 없이 캐시 없는 빌드로 확인).
WORKDIR /src
# 워크스페이스의 다른 멤버(crates/media, apps/desktop/src-tauri)도 Cargo.toml이 있어야 cargo가
# 워크스페이스를 읽는다. 서버만 빌드하므로 그 크레이트들은 컴파일하지 않는다. (.dockerignore 참고)
COPY . .

# 레지스트리·git·target을 캐시 마운트에 둔다. 바이너리는 캐시 밖(/out)으로 복사한다.
RUN --mount=type=cache,id=dongsi-cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=dongsi-cargo-git,target=/usr/local/cargo/git \
    --mount=type=cache,id=dongsi-target,target=/src/target \
    cargo build -p dongsi-server-app --release --locked \
 && install -D target/release/dongsi-server /out/dongsi-server

# ---- 실행 ----
FROM debian:trixie-slim

LABEL org.opencontainers.image.title="dongsi-server" \
      org.opencontainers.image.description="dongsi 전용 서버(화면 공유 시그널링 + SFU)" \
      org.opencontainers.image.source="https://github.com/YubinMoon/dongsi" \
      org.opencontainers.image.licenses="MIT"

# 비루트 사용자. uid/gid를 고정해 두면 호스트 폴더를 마운트할 때 chown 10001:10001로 맞출 수 있다.
RUN groupadd --system --gid 10001 dongsi \
 && useradd --system --uid 10001 --gid 10001 --no-create-home --home-dir /data --shell /usr/sbin/nologin dongsi \
 && mkdir /data \
 && chown 10001:10001 /data

COPY --from=build /out/dongsi-server /usr/local/bin/dongsi-server
# 라이선스(MIT)와 제3자 고지(서버에 들어가는 Rust 크레이트 포함. 서버는 FFmpeg를 쓰지 않는다). docs/licensing.md
COPY LICENSE THIRD_PARTY_NOTICES.md /usr/share/doc/dongsi-server/

ENV DONGSI_DATA=/data \
    DONGSI_IN_CONTAINER=1

# 인증서·키·설정(서버 신원). 위에서 소유자를 맞춘 뒤에 선언해야 새 볼륨이 dongsi 소유로 만들어진다.
VOLUME /data
USER dongsi
WORKDIR /data

# 같은 번호의 TCP(시그널링)와 UDP(미디어, 로컬 감지). 다른 포트를 쓰려면 DONGSI_PORT와 -p를 함께 바꾼다.
EXPOSE 7884/tcp 7884/udp

# docker stop은 SIGTERM을 보낸다. 서버는 접속자에게 닫힘을 알리고 종료 코드 0으로 끝난다.
STOPSIGNAL SIGTERM

# TCP 접속만 확인한다(TLS 핸드셰이크 전에 끊으므로 로그에 남지 않는다). 설정은 환경 변수·설정 파일에서 읽는다.
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["dongsi-server", "--healthcheck"]

ENTRYPOINT ["dongsi-server"]
