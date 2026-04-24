import { describe, it, expect } from 'vitest';
import { readFileSync } from 'fs';
import { resolve, dirname } from 'path';
import { fileURLToPath } from 'url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const jsonDir = resolve(__dirname, '../../src/lib/i18n');

function collectKeys(obj: Record<string, unknown>, prefix = ''): string[] {
  const keys: string[] = [];
  for (const [key, val] of Object.entries(obj)) {
    const full = prefix ? `${prefix}.${key}` : key;
    if (val && typeof val === 'object') {
      keys.push(...collectKeys(val as Record<string, unknown>, full));
    } else {
      keys.push(full);
    }
  }
  return keys;
}

describe('i18n locale key parity', () => {
  it('en.json and pt-BR.json have identical key sets', () => {
    const en = JSON.parse(readFileSync(resolve(jsonDir, 'en.json'), 'utf-8'));
    const ptBr = JSON.parse(readFileSync(resolve(jsonDir, 'pt-BR.json'), 'utf-8'));

    const enKeys = new Set(collectKeys(en));
    const ptBrKeys = new Set(collectKeys(ptBr));

    const missingInPtBr = [...enKeys].filter((k) => !ptBrKeys.has(k));
    const missingInEn = [...ptBrKeys].filter((k) => !enKeys.has(k));

    const problems: string[] = [];
    if (missingInPtBr.length > 0) {
      problems.push(`Missing in pt-BR.json: ${missingInPtBr.join(', ')}`);
    }
    if (missingInEn.length > 0) {
      problems.push(`Missing in en.json: ${missingInEn.join(', ')}`);
    }

    expect(problems, problems.length > 0 ? problems.join('\n') : undefined).toEqual([]);
  });
});
