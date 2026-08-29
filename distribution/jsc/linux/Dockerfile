ARG BASE_IMAGE
FROM ${BASE_IMAGE}

ARG APT_SNAPSHOT

ENV DEBIAN_FRONTEND=noninteractive

RUN test -n "${APT_SNAPSHOT}" \
    && apt-get --snapshot "${APT_SNAPSHOT}" update \
    && apt-get --snapshot "${APT_SNAPSHOT}" install -y --no-install-recommends \
        binutils \
        ca-certificates \
        clang-18 \
        cmake \
        git \
        jq \
        libicu-dev \
        lld-18 \
        ninja-build \
        patchelf \
        perl \
        pkg-config \
        python3 \
        ruby \
        zstd \
    && rm -rf /var/lib/apt/lists/*

ENV CC=clang-18 \
    CXX=clang++-18 \
    HOME=/tmp/kunlun-home

RUN mkdir -p /tmp/kunlun-home && chmod 1777 /tmp/kunlun-home
