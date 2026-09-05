import { readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';

const root = resolve(import.meta.dirname, '..');
const path = resolve(root, 'api/verifiers.json');
const verifiers = JSON.parse(readFileSync(path, 'utf8'));
const mappings = {
  'design-shotgun-landing-page': 'pulse-deck-directions',
  'programmatic-seo-integration-directory': 'relayops-integration-pages',
  'apple-design-interactive-sheet': 'queue-detail-sheet',
  'frontend-design-signal-dashboard': 'signal-desk-redesign',
  'marketing-ideas-constrained-launch': 'clipforge-launch-plan',
};
for (const [suite, task] of Object.entries(mappings)) {
  verifiers[suite][task] = readFileSync(resolve(root, `verifier_sources/${suite}.sh`), 'utf8');
}
writeFileSync(path, `${JSON.stringify(verifiers, null, 2)}\n`);
