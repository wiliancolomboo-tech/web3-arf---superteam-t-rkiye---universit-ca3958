const http = require("http");

const server = http.createServer((req, res) => {
  if (req.url === "/health" && req.method === "GET") {
    res.writeHead(200, { "Content-Type": "application/json" });
    return res.end(
      JSON.stringify({
        status: "UP",
        protocol: "ArfFlow",
        network: "Solana Devnet",
        uptimeSeconds: Math.floor(process.uptime()),
        timestamp: new Date().toISOString(),
      })
    );
  }
  res.writeHead(404);
  res.end();
});

const PORT = process.env.PORT || 8080;
server.listen(PORT, () => {
  console.log(`[ArfFlow Sentinel] Healthcheck server live on port ${PORT}`);
});