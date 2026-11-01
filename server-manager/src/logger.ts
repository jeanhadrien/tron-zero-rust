// Minimal tagged console logger. Level via LOG_LEVEL=debug|info|warn|error|none.
const LEVELS = { debug: 0, info: 1, warn: 2, error: 3, none: 4 } as const;
type Level = keyof typeof LEVELS;

const envLevel = (process.env.LOG_LEVEL ?? '').toLowerCase();
const threshold = envLevel in LEVELS ? LEVELS[envLevel as Level] : LEVELS.info;

export class Logger {
  constructor(private readonly tag: string) {}

  private emit(level: Exclude<Level, 'none'>, args: unknown[]): void {
    if (LEVELS[level] < threshold) return;
    console[level](`[${level.toUpperCase()}] [${this.tag}]`, ...args);
  }

  debug(...args: unknown[]): void {
    this.emit('debug', args);
  }
  info(...args: unknown[]): void {
    this.emit('info', args);
  }
  warn(...args: unknown[]): void {
    this.emit('warn', args);
  }
  error(...args: unknown[]): void {
    this.emit('error', args);
  }
}
