ARG BASE_IMAGE
FROM ${BASE_IMAGE}
ARG PH_TARGET
ARG BUILDER_UID=1000
ARG BUILDER_GID=1000
ENV PH_CI_CONTAINER=1
COPY packaging/ci/bootstrap.sh packaging/ci/dependencies.py /opt/peripheral-hub-packaging/ci/
COPY packaging/common.py packaging/targets.json /opt/peripheral-hub-packaging/
# BuildKit omits runtime markers; this marker belongs only to the dedicated SDK image.
RUN mkdir -p /run && touch /run/.containerenv && \
    /bin/sh /opt/peripheral-hub-packaging/ci/bootstrap.sh "${PH_TARGET}" && \
    if getent passwd "${BUILDER_UID}" >/dev/null; then userdel "$(getent passwd "${BUILDER_UID}" | cut -d: -f1)"; fi && \
    if getent group "${BUILDER_GID}" >/dev/null; then groupmod --new-name builder "$(getent group "${BUILDER_GID}" | cut -d: -f1)"; else groupadd --gid "${BUILDER_GID}" builder; fi && \
    useradd --uid "${BUILDER_UID}" --gid builder --create-home --shell /bin/bash builder && \
    mkdir -p /opt/peripheral-hub-toolchain /workspace && \
    chown builder:builder /opt/peripheral-hub-toolchain /workspace
USER builder
COPY packaging/ci/toolchain.py /opt/peripheral-hub-packaging/ci/toolchain.py
RUN python3 /opt/peripheral-hub-packaging/ci/toolchain.py --prefix /opt/peripheral-hub-toolchain
COPY packaging/audio_runtime.py packaging/pipewire.lock.json packaging/sources.py packaging/staging.py /opt/peripheral-hub-packaging/
RUN python3 /opt/peripheral-hub-packaging/audio_runtime.py \
    --rootfs /opt/peripheral-hub-toolchain/audio/rootfs --workdir /opt/peripheral-hub-toolchain/audio/work \
    --artifacts /opt/peripheral-hub-toolchain/audio/sources --cache /opt/peripheral-hub-toolchain/audio/cache
USER root
RUN cp -a /opt/peripheral-hub-toolchain/audio/rootfs/usr/. /usr/ && \
    chown -R root:root /usr/lib/peripheral-hub/pipewire /usr/share/doc/peripheral-hub/pipewire
USER builder
ENV PATH=/opt/peripheral-hub-toolchain/node/bin:/opt/peripheral-hub-toolchain/cargo/bin:$PATH
ENV CARGO_HOME=/opt/peripheral-hub-toolchain/cargo
ENV RUSTUP_HOME=/opt/peripheral-hub-toolchain/rustup
ENV HOME=/home/builder
WORKDIR /workspace
ENTRYPOINT ["python3", "/workspace/packaging/build.py"]
