import http from "http";

const PORT = process.env.PORT || 8080;

export function startHealthcheckServer() {
  const server = http.createServer((req, res) => {
    if (req.url === "/healthz") {
      res.writeHead(200, { "Content-Type": "application/json" });
      res.end(JSON.stringify({
        status: "UP",
        protocol: "Arf-Flow Solana Engine",
        timestamp: new Date().toISOString(),
        uptime_seconds: process.uptime()
      }));
    } else {
      res.writeHead(404);
      res.end();
    }
  });

  server.listen(PORT, () => {
    console.log(`📡 Healthcheck service listening on port ${PORT}`);
  });
}