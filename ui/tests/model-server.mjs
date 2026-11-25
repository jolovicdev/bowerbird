// A stand-in for a model server such as Ollama or LM Studio, for the browser
// tests that run without an API key. It speaks the OpenAI protocol the app
// uses for "Your own server": a model list, and chat replies streamed or
// whole. It keeps every chat request, so a test can check which model was
// asked and how.

import { createServer } from 'node:http';

const host = process.env.MODEL_SERVER_HOST ?? '127.0.0.2';
const port = Number(process.env.MODEL_SERVER_PORT ?? 7494);

/** In Ollama's name:tag form, which the app sends to the user's own server. */
const MODELS = ['stand-in-writer:1b', 'stand-in-thinker:1b', 'stand-in-spare:1b'];

/** Chat requests so far: the model, the reasoning effort asked for, and the question. */
let asked = [];

/** An answer that quotes the first passage in the prompt, so its citation is found in the source. */
function answer(prompt) {
	const passage = /\n\[([A-Z]+\d+)\][^\n]*\n([^\n]+)/.exec(prompt);
	if (!passage) return 'OK';
	const quote = passage[2].split(/\s+/).slice(0, 6).join(' ').replace(/[.,;:]+$/, '');
	return `The sources say this: ${quote} [[${passage[1]}|${quote}]].`;
}

const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function chat(request, response) {
	let body = '';
	for await (const piece of request) body += piece;
	const { model, messages, stream, reasoning_effort: effort } = JSON.parse(body);
	const prompt = messages.at(-1).content;
	const text = typeof prompt === 'string' ? prompt : JSON.stringify(prompt);
	asked.push({ model, effort: effort ?? null, question: /Question: (.*)/.exec(text)?.[1] ?? text.slice(0, 80) });
	const reply = answer(text);
	if (!stream) {
		response.writeHead(200, { 'Content-Type': 'application/json' });
		response.end(JSON.stringify({ choices: [{ index: 0, message: { role: 'assistant', content: reply }, finish_reason: 'stop' }] }));
		return;
	}
	response.writeHead(200, { 'Content-Type': 'text/event-stream' });
	const send = (delta, finish = null) =>
		response.write(`data: ${JSON.stringify({ choices: [{ index: 0, delta, finish_reason: finish }] })}\n\n`);
	if (model.includes('thinker')) {
		// Long enough for a test to see the thinking before the answer takes its place.
		send({ reasoning_content: 'The question is about the first passage. ' });
		await pause(1500);
		send({ reasoning_content: 'A short quote from it will do.' });
		await pause(1500);
	}
	for (const word of reply.split(/(?<= )/)) send({ content: word });
	send({}, 'stop');
	response.end('data: [DONE]\n\n');
}

createServer((request, response) => {
	const { pathname } = new URL(request.url, `http://${host}`);
	if (request.method === 'GET' && pathname === '/v1/models') {
		response.writeHead(200, { 'Content-Type': 'application/json' });
		response.end(JSON.stringify({ data: MODELS.map((id) => ({ id })) }));
	} else if (request.method === 'POST' && pathname === '/v1/chat/completions') {
		chat(request, response).catch((e) => response.destroy(e));
	} else if (pathname === '/asked') {
		if (request.method === 'DELETE') asked = [];
		response.writeHead(200, { 'Content-Type': 'application/json' });
		response.end(JSON.stringify(asked));
	} else {
		response.writeHead(404).end();
	}
}).listen(port, host);
