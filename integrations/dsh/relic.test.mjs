import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { resolve, join } from 'node:path';
import { apply } from './relic.mjs';

test('native DSH adapter retrieves context and captures only current completed turn', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'relic-dsh-'));
  try {
    const executable = resolve('target/debug/relic'), vault = join(dir, 'vault space');
    execFileSync(executable, ['init', vault]);
    const handlers = {}, warnings = [];
    apply({ on: (name, handler) => { handlers[name] = handler; }, logger: { warn: value => warnings.push(value) } }, { executable, vault, project: dir });
    const agent = { session: { header: { id: 'dsh-session', cwd: dir }, snapshotEvents: () => [
      { type: 'assistant/message', data: { turn: 1, message: { content: [{type:'text',text:'OLD'}] } } },
      { type: 'assistant/message', data: { turn: 2, message: { content: [{type:'text',text:'NEW'}] } } },
      { type: 'assistant/message', data: { turn: 2, interrupted:true, message: { content: [{type:'text',text:'INTERRUPTED'}] } } },
    ] } };
    const outside = { session: { ...agent.session, header: { id:'outside', cwd:tmpdir() } } };
    const skipped = await handlers['agent/pre-step']({agent:outside,turn:2,messages:[{content:[]}]},async()=>({kind:'enter',messages:[]}));
    assert.deepEqual(skipped.messages,[]);
    await handlers['agent/turn-stopping']({agent:outside,turn:2});
    const result = await handlers['agent/pre-step']({agent,turn:2,messages:[{content:[{type:'text',text:'Review recovery'}]}]},async () => ({kind:'enter',messages:[]}));
    assert.equal(result.messages[0].source.plugin,'relic-memory');
    await handlers['agent/turn-stopping']({agent,turn:2});
    await handlers['agent/turn-stopping']({agent,turn:2});
    assert.deepEqual(warnings,[]);
    const queue = JSON.parse(execFileSync(executable,['queue','list'],{cwd:vault,encoding:'utf8'}));
    assert.equal(queue.length,1);
    const capture = JSON.parse(execFileSync(executable,['queue','get',queue[0].id],{cwd:vault,encoding:'utf8'}));
    assert.equal(capture.source.input.outcome,'NEW');
  } finally { rmSync(dir,{recursive:true,force:true}); }
});
