import assert from 'node:assert/strict';
import {execFileSync} from 'node:child_process';
import {readFileSync} from 'node:fs';
import {resolve} from 'node:path';
import test from 'node:test';

const root = resolve(import.meta.dirname, '..');
const verifiers = JSON.parse(readFileSync(resolve(root, 'api/verifiers.json'), 'utf8'));
const mappings = {
  'design-shotgun-landing-page': 'pulse-deck-directions',
  'programmatic-seo-integration-directory': 'relayops-integration-pages',
  'apple-design-interactive-sheet': 'queue-detail-sheet',
  'frontend-design-signal-dashboard': 'signal-desk-redesign',
  'marketing-ideas-constrained-launch': 'clipforge-launch-plan',
};

for (const [suite, task] of Object.entries(mappings)) {
  test(`${suite} verifier source is valid shell and matches the API payload`, () => {
    const sourcePath = resolve(root, `verifier_sources/${suite}.sh`);
    const source = readFileSync(sourcePath, 'utf8');
    execFileSync('bash', ['-n', sourcePath]);
    assert.equal(verifiers[suite][task], source);
  });
}
