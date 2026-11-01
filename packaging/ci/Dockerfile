ARG BASE_IMAGE
FROM ${BASE_IMAGE}
ARG PH_TARGET
ARG BUILDER_UID=1000
ENV PH_CI_CONTAINER=1
COPY packaging /opt/peripheral-hub-packaging
RUN /bin/sh /opt/peripheral-hub-packaging/ci/bootstrap.sh "${PH_TARGET}" && \
    useradd --user-group --non-unique --uid "${BUILDER_UID}" --create-home --shell /bin/bash builder && \
    mkdir -p /opt/peripheral-hub-toolchain /workspace && \
    chown builder:builder /opt/peripheral-hub-toolchain /workspace
USER builder
RUN python3 /opt/peripheral-hub-packaging/ci/toolchain.py --prefix /opt/peripheral-hub-toolchain
ENV PATH=/opt/peripheral-hub-toolchain/node/bin:/opt/peripheral-hub-toolchain/cargo/bin:$PATH
ENV CARGO_HOME=/opt/peripheral-hub-toolchain/cargo
ENV RUSTUP_HOME=/opt/peripheral-hub-toolchain/rustup
WORKDIR /workspace
ENTRYPOINT ["python3", "/workspace/packaging/build.py"]
