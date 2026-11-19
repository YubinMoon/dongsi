# dongsi용 최소 LGPL ffmpeg 교차 빌드 환경 (Windows x64용 mingw-w64, 스레드 모델 posix)
# 기반 이미지는 다이제스트로 고정한다. 바꾸려면 `docker pull ubuntu:24.04` 뒤
# `docker inspect --format '{{index .RepoDigests 0}}' ubuntu:24.04`로 다이제스트를 얻는다.
FROM ubuntu:24.04@sha256:73b999393d07f2bf65a40373778a44dc9a6d54ad7718d29247d8ff9413c4d6a8

ENV DEBIAN_FRONTEND=noninteractive

# apt 패키지 버전은 고정하지 않는다(우분투 24.04 저장소의 그 시점 버전). 실제로 쓴 버전은 BUILDINFO.md에 남는다.
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates \
      build-essential make pkg-config cmake meson ninja-build nasm \
      gcc-mingw-w64-x86-64-posix g++-mingw-w64-x86-64-posix \
      binutils-mingw-w64-x86-64 mingw-w64-x86-64-dev mingw-w64-tools \
      python3 xz-utils file diffutils patch perl \
 && rm -rf /var/lib/apt/lists/*

# x86_64-w64-mingw32-gcc/g++가 posix 스레드 변형을 가리키게 한다.
RUN update-alternatives --set x86_64-w64-mingw32-gcc /usr/bin/x86_64-w64-mingw32-gcc-posix \
 && update-alternatives --set x86_64-w64-mingw32-g++ /usr/bin/x86_64-w64-mingw32-g++-posix \
 && x86_64-w64-mingw32-gcc -v 2>&1 | grep -q 'Thread model: posix' \
 && x86_64-w64-mingw32-g++ -v 2>&1 | grep -q 'Thread model: posix'

WORKDIR /work
