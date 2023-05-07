FROM alpine
ARG TARGETARCH
COPY /${TARGETARCH}-executables/hypersonic /usr/bin/

ENTRYPOINT "/usr/bin/hypersonic"