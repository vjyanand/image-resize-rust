FROM alpine:latest as builder

RUN apk add --update --no-cache --repository https://dl-3.alpinelinux.org/alpine/latest-stable/community --repository https://dl-3.alpinelinux.org/alpine/latest-stable/main rust cargo openssl-dev libjpeg-turbo-utils libpng

WORKDIR /app

COPY ./ ./

RUN cargo test

RUN cargo build --release

FROM alpine:latest

RUN apk add --update --no-cache --repository https://dl-3.alpinelinux.org/alpine/latest-stable/community --repository https://dl-3.alpinelinux.org/alpine/latest-stable/main libgcc libjpeg-turbo-utils libpng

WORKDIR /app

COPY --from=builder /app/target/release/image /app/image

EXPOSE 8080

#Run the binary
CMD ["/app/image"]
