import * as http from "http";

const HEALTH_PORT = process.env.PORT || 8080;
const server = http.createServer((req, res) => {
  if (req.url === "/healthz" || req.url === "/metrics") {
    res.writeHead(200, { "Content-Type": "application/json" });
    res.end(
      JSON.stringify({
        status: "UP",
        service: "ArfFlow-Telemetry-Broker",
        uptime_seconds: process.uptime(),
        timestamp: new Date().toISOString(),
      })
    );
  } else {
    res.writeHead(404);
    res.end();
  }
});
server.listen(HEALTH_PORT, () => {
  console.log(`🩺 Healthcheck service listening on port ${HEALTH_PORT}`);
});