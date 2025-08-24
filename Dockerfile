FROM alpine:latest as builder

RUN apk add --update --no-cache --repository https://dl-3.alpinelinux.org/alpine/latest-stable/community --repository https://dl-3.alpinelinux.org/alpine/latest-stable/main rust cargo openssl-dev

WORKDIR /app

COPY ./ ./

#RUN cargo test

RUN cargo build --release

FROM alpine:latest

RUN apk add --update --no-cache --repository https://dl-3.alpinelinux.org/alpine/latest-stable/community --repository https://dl-3.alpinelinux.org/alpine/latest-stable/main libgcc

WORKDIR /app

COPY --from=builder /app/target/release/image /app/image

ENV RUST_LOG=info,reqwest=warn,hyper_util::client::legacy::client=warn,hyper_util::client::legacy::connect::http=warn,hyper_util::client::legacy::pool=warn,hyper_util::client::=warn,hyper_util::client::legacy::connect::dns=warn
   
EXPOSE 8080

#Run the binary
CMD ["/app/image"]
