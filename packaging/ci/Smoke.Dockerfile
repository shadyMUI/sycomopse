ARG BASE_IMAGE
FROM ${BASE_IMAGE}
ARG PH_TARGET
ENV PH_CI_CONTAINER=1
COPY packaging/ci/bootstrap.sh packaging/ci/dependencies.py /opt/peripheral-hub-packaging/ci/
COPY packaging/common.py packaging/targets.json /opt/peripheral-hub-packaging/
RUN mkdir -p /run && touch /run/.containerenv && \
    /bin/sh /opt/peripheral-hub-packaging/ci/bootstrap.sh "${PH_TARGET}" --runtime-only && \
    if getent passwd 1000 >/dev/null; then userdel "$(getent passwd 1000 | cut -d: -f1)"; fi && \
    if getent group 1000 >/dev/null; then groupmod --new-name builder "$(getent group 1000 | cut -d: -f1)"; else groupadd --gid 1000 builder; fi && \
    useradd --uid 1000 --gid builder --create-home --shell /bin/bash builder
ENTRYPOINT ["/usr/lib/systemd/systemd"]
