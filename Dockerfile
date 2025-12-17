FROM alpine:latest AS builder

RUN apk add --update --no-cache --repository https://dl-3.alpinelinux.org/alpine/latest-stable/community --repository https://dl-3.alpinelinux.org/alpine/latest-stable/main rust cargo openssl-dev

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

RUN apk add --update --no-cache --repository https://dl-3.alpinelinux.org/alpine/latest-stable/community --repository https://dl-3.alpinelinux.org/alpine/latest-stable/main libgcc

WORKDIR /app

COPY --from=builder /app/target/release/image /app/image

ENV RUST_LOG=info,reqwest=warn,hyper_util::client::legacy::client=warn,hyper_util::client::legacy::connect::http=warn,hyper_util::client::legacy::pool=warn,hyper_util::client::=warn,hyper_util::client::legacy::connect::dns=warn
   
EXPOSE 8080

#Run the binary
CMD ["/app/image"]
