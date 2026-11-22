ARG SDK_IMAGE=peripheral-hub-sdk:ubuntu-24.04-98bffd516864
FROM ${SDK_IMAGE}
USER root
RUN apt-get update && apt-get install --yes --no-install-recommends \
    qemu-system-x86 cloud-image-utils genisoimage ovmf gpgv ubuntu-cloudimage-keyring && \
    test -r /usr/share/keyrings/ubuntu-cloudimage-keyring.gpg && \
    test -r /usr/share/OVMF/OVMF_CODE_4M.fd
LABEL org.peripheralhub.vm-role="ubuntu-24.04-tcg" \
      org.peripheralhub.sdk-base="sha256:98bffd5168643b1f2d9d74e9e572983061536f84bc79ac090a8ba21eb98a4af7"
ENV PH_CI_VM_BUILDER=1
USER builder
WORKDIR /workspace
ENTRYPOINT ["python3", "-B", "/workspace/packaging/ci/vm.py", "--worker"]
