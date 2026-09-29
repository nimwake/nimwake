// Nimwake 예제 애플리케이션.
//
// manifest(`nimwake.toml`)의 계약은 한 문장이다:
// "이 command를 실행하면 이 port에서 HTTP application이 listen한다."

const http = require("node:http");

const port = Number(process.env.PORT ?? 3101);

http
  .createServer((request, response) => {
    response.writeHead(200, { "content-type": "text/plain; charset=utf-8" });
    response.end(`hello from nimwake (pid ${process.pid})\n`);
  })
  .listen(port, "127.0.0.1", () => {
    console.log(`hello-node listening on 127.0.0.1:${port}`);
  });

for (const signal of ["SIGTERM", "SIGINT"]) {
  process.on(signal, () => process.exit(0));
}
