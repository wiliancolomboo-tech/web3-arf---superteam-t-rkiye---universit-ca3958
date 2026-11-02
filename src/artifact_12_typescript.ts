import http from "http";
import * as dotenv from "dotenv";

dotenv.config();

const PORT = process.env.PORT || 8080;

const server = http.createServer((req, res) => {
  if (req.url === "/health" && req.method === "GET") {
    const healthStatus = {
      status: "HEALTHY",
      uptime: process.uptime(),
      timestamp: new Date().toISOString(),
      solanaCluster: process.env.SOLANA_RPC_URL || "https://api.devnet.solana.com",
      protocol: "ArfFlow Core v1.0.0",
      services: {
        slackWebhook: !!process.env.SLACK_WEBHOOK_URL,
        resendEngine: !!process.env.RESEND_API_KEY,
      },
    };
    res.writeHead(200, { "Content-Type": "application/json" });
    res.end(JSON.stringify(healthStatus));
  } else {
    res.writeHead(404, { "Content-Type": "application/json" });
    res.end(JSON.stringify({ error: "Endpoint not found" }));
  }
});

server.listen(PORT, () => {
  console.log(`[ARFFLOW-SERVER] Operational Telemetry listening on port ${PORT}`);
});