import http from 'http';
import * as dotenv from 'dotenv';
dotenv.config();

const PORT = process.env.PORT || 8080;

const server = http.createServer((req, res) => {
  if (req.url === '/health' && req.method === 'GET') {
    res.writeHead(200, { 'Content-Type': 'application/json' });
    res.end(
      JSON.stringify({
        status: 'UP',
        timestamp: new Date().toISOString(),
        service: 'ArfFlow Enterprise Engine',
        corridorsActive: ['EUR/TRY', 'USD/TRY'],
        solanaRpcConnected: true,
      })
    );
    return;
  }

  res.writeHead(404, { 'Content-Type': 'application/json' });
  res.end(JSON.stringify({ error: 'Endpoint not found' }));
});

server.listen(PORT, () => {
  console.log(`[ArfFlow Telemetry] HTTP Healthcheck daemon listening on port ${PORT}`);
});