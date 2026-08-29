ARG BASE_IMAGE
ARG TRUST_STORE_IMAGE

FROM ${TRUST_STORE_IMAGE} AS trust-store

FROM ${BASE_IMAGE}

ARG APT_SNAPSHOT

ENV DEBIAN_FRONTEND=noninteractive

COPY --from=trust-store /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt

RUN test -n "${APT_SNAPSHOT}" \
    && printf '%s\n' \
        'Types: deb' \
        "URIs: https://snapshot.ubuntu.com/ubuntu/${APT_SNAPSHOT}" \
        'Suites: noble noble-updates noble-backports noble-security' \
        'Components: main restricted universe multiverse' \
        'Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg' \
        'Check-Valid-Until: no' \
        > /etc/apt/sources.list.d/ubuntu.sources \
    && apt-get update \
    && apt-get install -y --no-install-recommends \
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
    && grep -F "URIs: https://snapshot.ubuntu.com/ubuntu/${APT_SNAPSHOT}" /etc/apt/sources.list.d/ubuntu.sources \
    && rm -rf /var/lib/apt/lists/*

ENV CC=clang-18 \
    CXX=clang++-18 \
    HOME=/tmp/kunlun-home

RUN mkdir -p /tmp/kunlun-home && chmod 1777 /tmp/kunlun-home
