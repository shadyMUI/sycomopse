ARG BASE_IMAGE=peripheral-hub-sdk@sha256:98bffd5168643b1f2d9d74e9e572983061536f84bc79ac090a8ba21eb98a4af7
FROM ${BASE_IMAGE}
USER root
RUN apt-get update && apt-get install --yes --no-install-recommends meson libsndfile1-dev patchelf && \
    mkdir -p /opt/peripheral-hub-audio && chown builder:builder /opt/peripheral-hub-audio
COPY packaging/audio_runtime.py packaging/pipewire.lock.json packaging/common.py packaging/sources.py packaging/staging.py /opt/peripheral-hub-packaging/
USER builder
RUN python3 /opt/peripheral-hub-packaging/audio_runtime.py \
    --rootfs /opt/peripheral-hub-audio/rootfs --workdir /opt/peripheral-hub-audio/work \
    --artifacts /opt/peripheral-hub-audio/sources --cache /opt/peripheral-hub-audio/cache
USER root
RUN cp -a /opt/peripheral-hub-audio/rootfs/usr/. /usr/ && \
    chown -R root:root /usr/lib/peripheral-hub/pipewire /usr/share/doc/peripheral-hub/pipewire
USER builder
