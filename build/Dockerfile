FROM debian:13.1-slim@sha256:a347fd7510ee31a84387619a492ad6c8eb0af2f2682b916ff3e643eb076f925a
ARG DEBIAN_SNAPSHOT
# The add-on links EGL and GLES from Debian; the box's Mali drivers provide them at runtime.
# The native ones let its unit tests link on the build machine.
RUN rm -f /etc/apt/sources.list.d/* && \
    printf 'deb [check-valid-until=no] http://snapshot.debian.org/archive/debian/%s/ trixie main\n' "$DEBIAN_SNAPSHOT" > /etc/apt/sources.list && \
    arm64=; cross=; \
    if [ "$(dpkg --print-architecture)" != arm64 ]; then \
        dpkg --add-architecture arm64; arm64=:arm64; cross="gcc-aarch64-linux-gnu libc6-dev-arm64-cross"; \
    fi && \
    apt-get update && apt-get install -y --no-install-recommends \
        $cross gcc libc6-dev bindgen clang ca-certificates curl python3 rustup shellcheck xz-utils \
        libegl-dev libgles-dev libegl-dev$arm64 libgles-dev$arm64 && \
    rm -rf /var/lib/apt/lists/*
ENV LANG=C.UTF-8
WORKDIR /work
