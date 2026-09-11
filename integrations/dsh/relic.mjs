// Native DSH 0.1.2-rc.1 adapter. No transcript scraping or forced continuation.
import { spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { isAbsolute, resolve, sep } from 'node:path';
export const name = 'relic-memory';
export const inject = ['sessionProjections'];
const text = blocks => (blocks || []).filter(b => b.type === 'text').map(b => b.text).join('\n');
export function invoke(config, input) {
  return new Promise((resolve, reject) => {
    const child = spawn(config.executable, ['hook', '--host', 'dsh', '--vault', config.vault], { shell: false, stdio: ['pipe', 'pipe', 'pipe'] });
    let output = '', error = '';
    const timer = setTimeout(() => { child.kill(); reject(new Error('Relic hook timed out')); }, 10000);
    child.on('error', e => { clearTimeout(timer); reject(e); });
    child.stdin.on('error', () => {});
    child.stdout.on('data', chunk => { output += chunk; if (output.length > 65536) child.kill(); });
    child.stderr.on('data', chunk => { error = (error + chunk).slice(-2000); });
    child.on('close', code => {
      clearTimeout(timer);
      if (code !== 0) return reject(new Error(error || `Relic exited ${code}`));
      try { resolve(JSON.parse(output)); } catch (e) { reject(e); }
    });
    child.stdin.end(JSON.stringify(input));
  });
}
export function apply(ctx, config) {
  if (!isAbsolute(config.executable || '') || !isAbsolute(config.vault || '')) throw new Error('Relic executable and vault must be absolute paths');
  const inScope = agent => !config.project || resolve(agent.session.header.cwd) === resolve(config.project) || resolve(agent.session.header.cwd).startsWith(resolve(config.project) + sep);
  const base = (agent, turn, event) => ({ session_id: agent.session.header.id, cwd: agent.session.header.cwd, turn_id: String(turn), hook_event_name: event });
  ctx.on('agent/pre-step', async ({ agent, messages, turn }, next) => {
    if (!messages.length || !inScope(agent)) return next();
    let context;
    try {
      const result = await invoke(config, { ...base(agent, turn, 'UserPromptSubmit'), prompt: text(messages.flatMap(m => m.content)).slice(0, 8000) });
      context = result.hookSpecificOutput?.additionalContext;
    } catch (e) { ctx.logger.warn(`Relic retrieval failed: ${e.message}`); }
    const downstream = await next();
    if (!context || downstream.kind !== 'enter') return downstream;
    // DSH's public UserMessage value contract: stable UUID and frozen provenance.
    const message = Object.freeze({ id: randomUUID(), role: 'user', source: Object.freeze({ kind: 'plugin', plugin: name }), content: Object.freeze([Object.freeze({ type: 'text', text: context })]) });
    return { ...downstream, messages: [...downstream.messages, message] };
  });
  ctx.on('agent/turn-stopping', async ({ agent, turn }) => {
    if (!inScope(agent)) return;
    const event = agent.session.snapshotEvents().findLast(e => e.type === 'assistant/message' && e.data.turn === turn && !e.data.interrupted);
    if (!event) return;
    try {
      await invoke(config, { ...base(agent, turn, 'Stop'), last_assistant_message: text(event.data.message.content).slice(0, 24000) });
    } catch (e) { ctx.logger.warn(`Relic capture failed: ${e.message}`); }
  });
}
