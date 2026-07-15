# ---------- Build stage ----------
FROM rust:alpine AS builder

RUN apk add --no-cache \
    musl-dev \
    build-base

WORKDIR /app

COPY . .

RUN cargo build --release --bin papr

# ---------- Runtime stage ----------
FROM alpine:latest

ARG USERNAME
ARG UID
ARG GID

RUN apk add --no-cache \
    bash \
    vim \
    ca-certificates && \
    addgroup -g ${GID} ${USERNAME} && \
    adduser -D -u ${UID} -G ${USERNAME} -s /bin/bash ${USERNAME}

COPY --from=builder /app/target/release/papr /usr/local/bin/papr

USER ${USERNAME}
WORKDIR /home/${USERNAME}

CMD ["bash"]
