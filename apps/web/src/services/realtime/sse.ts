// Incremental Server-Sent Events parser (the subset the realtime service emits:
// `event`, `id`, `data`, comments). Feed it text as it arrives.

export interface SseMessage {
  event: string;
  id?: string;
  data: string;
}

export class SseParser {
  private buffer = "";

  push(text: string): SseMessage[] {
    this.buffer += text.replace(/\r\n?/g, "\n");
    const out: SseMessage[] = [];
    let end: number;
    while ((end = this.buffer.indexOf("\n\n")) >= 0) {
      const block = this.buffer.slice(0, end);
      this.buffer = this.buffer.slice(end + 2);
      const message: SseMessage = { event: "message", data: "" };
      const data: string[] = [];
      let meaningful = false;
      for (const line of block.split("\n")) {
        if (!line || line.startsWith(":")) continue;
        const colon = line.indexOf(":");
        const field = colon < 0 ? line : line.slice(0, colon);
        const value = colon < 0 ? "" : line.slice(colon + 1).replace(/^ /, "");
        if (field === "event") message.event = value;
        else if (field === "id") message.id = value;
        else if (field === "data") data.push(value);
        else continue;
        meaningful = true;
      }
      if (!meaningful) continue; // keep-alive comment
      message.data = data.join("\n");
      out.push(message);
    }
    return out;
  }
}
