FROM alpine:latest AS builder

RUN apk add --update --no-cache --repository https://dl-3.alpinelinux.org/alpine/latest-stable/community --repository https://dl-3.alpinelinux.org/alpine/latest-stable/main rust cargo openssl-dev dav1d-dev

WORKDIR /opt/breaking

# Copy Cargo files for caching
COPY Cargo.toml ./

# Dummy src for deps
RUN mkdir src && echo "fn main() {}" > src/main.rs

# Build deps with target (caches musl artifacts)
RUN cargo build --release && rm -rf src

# Copy real src
COPY src ./src

# Final build (touch to trigger rebuild)
RUN touch src/main.rs && cargo build --release

FROM alpine:latest

RUN apk add --update --no-cache --repository https://dl-3.alpinelinux.org/alpine/latest-stable/community --repository https://dl-3.alpinelinux.org/alpine/latest-stable/main libgcc dav1d

WORKDIR /app

COPY --from=builder /opt/breaking/target/release/image /app/image

ENV FALL_BACK_URL=https://webkit.extruct.iavian.net/webkit/proxy_basic?url=

ENV RUST_LOG=image=debug
   
EXPOSE 8080

#Run the binary
CMD ["/app/image"]
