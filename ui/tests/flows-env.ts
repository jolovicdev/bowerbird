// Where the keyless browser tests run: the app, and the stand-in model server
// it is pointed at. Not 127.0.0.1, for the reason given in playwright.config.ts.

export const HOST = '127.0.0.2';
export const APP_PORT = 7492;
export const MODEL_PORT = 7494;
export const TOKEN = 'flows';
export const MODEL_SERVER = `http://${HOST}:${MODEL_PORT}`;
