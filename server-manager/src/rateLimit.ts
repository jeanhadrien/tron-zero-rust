import type { Request, Response, NextFunction } from 'express';

// Fixed-window per-IP limiter; expired windows are pruned periodically so
// memory stays bounded.
export function rateLimit(limit: number, windowMs: number) {
  const hits = new Map<string, { windowStart: number; count: number }>();

  const prune = setInterval(() => {
    const now = Date.now();
    for (const [ip, h] of hits) if (now - h.windowStart >= windowMs) hits.delete(ip);
  }, windowMs);
  prune.unref();

  return (req: Request, res: Response, next: NextFunction): void => {
    const ip = req.ip ?? 'unknown';
    const now = Date.now();
    let h = hits.get(ip);
    if (!h || now - h.windowStart >= windowMs) {
      h = { windowStart: now, count: 0 };
      hits.set(ip, h);
    }
    h.count++;
    if (h.count > limit) {
      res.set('Retry-After', String(Math.ceil((h.windowStart + windowMs - now) / 1000)));
      res.status(429).json({ error: 'Too many registrations' });
      return;
    }
    next();
  };
}
