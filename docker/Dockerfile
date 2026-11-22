# The two images hypha publishes, one stage each. docker/build.sh builds them by --target:
#   builder   Ubuntu with Rust, CMake, GCC, chisel and the gate tools, per --platform.
#             Hypha's CI runs in it, and applications build hypha from source in it.
#             An application adds its own libraries in an image FROM it.
#   runtime   chiselled Ubuntu with glibc, libstdc++ and a busybox shell, per
#             --platform. Applications copy their install tree onto it.
#
# Both carry only what hypha needs. Build context: docker/. Named context "ca": a
# directory holding ca-bundle.crt, the trust bundle the builds fetch through.
#
# Both are Ubuntu 26.04, so build and target agree on glibc and libstdc++.
ARG HYPHA_UBUNTU_IMAGE=ubuntu:26.04@sha256:da6fc2be547864451aa253836dd926da33623312df4a9a243e35dc877c378a78

FROM ${HYPHA_UBUNTU_IMAGE} AS builder

ENV DEBIAN_FRONTEND=noninteractive
ENV RUSTUP_HOME=/usr/local/rustup
ENV CARGO_HOME=/usr/local/cargo
ENV PATH=/usr/local/cargo/bin:$PATH

ARG HYPHA_RUST_VERSION=1.98.0
ARG HYPHA_CHISEL_VERSION=v1.5.0
# scripts/format.sh accepts exactly this clang-format major. The PyPI wheel
# tracks the LLVM release on every host, the apt package does not.
ARG HYPHA_CLANG_FORMAT_MAJOR=23
# The exact release: another minor of one major has formatted the tree differently.
ARG HYPHA_CLANG_FORMAT_VERSION=23.1.5

COPY --from=ca ca-bundle.crt /usr/local/share/ca-certificates/hypha-ca.crt

RUN echo 'Acquire::Retries "5";' > /etc/apt/apt.conf.d/80-retries \
    && echo 'precedence ::ffff:0:0/96  100' >> /etc/gai.conf

# lsof lets clean-shm.sh find the processes that still hold shared memory;
# tmux runs hypha_launch sessions; Eigen builds the MRS example that test.sh flies;
# PCL builds the Autoware port (examples/autoware).
RUN apt-get update && apt-get install -y --no-install-recommends \
        binutils \
        bzip2 \
        ca-certificates \
        clang-tidy \
        cmake \
        curl \
        g++ \
        gcc \
        git \
        libeigen3-dev \
        libgtest-dev \
        libpcl-dev \
        libssl-dev \
        lsof \
        make \
        pkgconf \
        python3 \
        python3-pip \
        ripgrep \
        rustup \
        shellcheck \
        tmux \
    && update-ca-certificates \
    && rm -rf /var/lib/apt/lists/*

ENV SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt
ENV CARGO_HTTP_CAINFO=/etc/ssl/certs/ca-certificates.crt
ENV GIT_SSL_CAINFO=/etc/ssl/certs/ca-certificates.crt
ENV REQUESTS_CA_BUNDLE=/etc/ssl/certs/ca-certificates.crt

RUN pip install --no-cache-dir --break-system-packages \
        "clang-format==${HYPHA_CLANG_FORMAT_VERSION}" \
        "mypy==1.14.1" "ruff==0.17.6" \
    && clang-format --version | grep -q "version ${HYPHA_CLANG_FORMAT_MAJOR}\."

# rust-nightly names the dated nightly of scripts/miri.sh and scripts/fuzz.sh, the
# only users of a nightly toolchain; the scripts read the same file.
COPY rust-nightly /usr/local/share/hypha/rust-nightly
RUN mkdir -p "$CARGO_HOME" "$RUSTUP_HOME" \
    && rustup toolchain install "${HYPHA_RUST_VERSION}" --profile minimal -c clippy -c rustfmt \
    && rustup default "${HYPHA_RUST_VERSION}" \
    && rustup component add llvm-tools-preview \
    && rustup toolchain install "$(cat /usr/local/share/hypha/rust-nightly)" --profile minimal \
         -c miri -c rust-src \
    && cargo install --locked cbindgen@0.29.4 cargo-nextest@0.9.152 cargo-llvm-cov@0.9.1 \
         cargo-deny@0.22.0 cargo-fuzz@0.13.2 \
    && rm -rf "$CARGO_HOME/registry" "$CARGO_HOME/git" \
    && chmod -R a+rwX "$CARGO_HOME" "$RUSTUP_HOME"

# chisel cuts the runtime base below, and an application's runtime libraries.
RUN set -eux; \
    arch="$(dpkg --print-architecture)"; \
    base="https://github.com/canonical/chisel/releases/download/${HYPHA_CHISEL_VERSION}"; \
    file="chisel_${HYPHA_CHISEL_VERSION}_linux_${arch}.tar.gz"; \
    cd /tmp; \
    curl -sSfL --retry 5 --retry-delay 2 --retry-all-errors -O "${base}/${file}"; \
    curl -sSfL --retry 5 --retry-delay 2 --retry-all-errors -O "${base}/${file}.sha384"; \
    sha384sum -c "${file}.sha384"; \
    tar -xzf "${file}" -C /usr/local/bin chisel; \
    rm -f "${file}" "${file}.sha384"; \
    chisel version

WORKDIR /work
CMD ["/bin/bash"]

# glibc, libstdc++ and a busybox shell, cut for the target architecture.
FROM builder AS rootfs
ARG TARGETARCH
RUN . /etc/os-release \
    && mkdir -p /rootfs \
    && chisel cut --release "ubuntu-${VERSION_ID}" --arch "${TARGETARCH}" --root /rootfs \
         base-files_base \
         libc6_libs \
         libgcc-s1_libs \
         libstdc++6_libs \
         busybox_bins \
    && for applet in sh ls cat ps top grep find df du kill sleep hexdump; do \
         ln -s busybox "/rootfs/usr/bin/$applet"; \
       done

# The busybox package is the binary alone. The applets above are the ones a node
# is debugged with; everything else stays reachable as "busybox <applet>".
FROM scratch AS runtime
COPY --from=rootfs /rootfs /
ENV PATH=/usr/local/sbin:/usr/local/bin:/usr/bin:/usr/sbin:/sbin:/bin
WORKDIR /work
CMD ["/bin/sh"]
