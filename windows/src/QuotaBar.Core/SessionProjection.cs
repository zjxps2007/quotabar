using System.Text;
using System.Text.Json.Nodes;

namespace QuotaBar.Core;

// Retain only usage metadata. Large prompts, tools and response bodies are skipped
// character by character; neither a whole JSONL row nor its content is allocated.
public sealed class SessionProjection(TextReader reader, CancellationToken token)
{
    private static readonly Dictionary<string, HashSet<string>> Fields = new()
    {
        [""] = ["type", "timestamp", "payload", "message"],
        ["payload"] = ["type", "id", "forked_from_id", "forkedFromId", "timestamp", "model", "info"],
        ["payload.info"] = ["model", "model_name", "total_token_usage", "last_token_usage"],
        ["payload.info.total_token_usage"] = ["input_tokens", "cached_input_tokens", "output_tokens"],
        ["payload.info.last_token_usage"] = ["input_tokens", "cached_input_tokens", "output_tokens"],
        ["message"] = ["id", "model", "usage"],
        ["message.usage"] = ["input_tokens", "cache_read_input_tokens", "cache_creation_input_tokens", "output_tokens"]
    };
    private int reads;
    private int Read() { if ((++reads & 4095) == 0) token.ThrowIfCancellationRequested(); return reader.Read(); }
    private void Space() { while (reader.Peek() is 32 or 9 or 13) Read(); }
    private int Required() { int c = Read(); if (c < 0) throw new EndOfStreamException(); if (c == '\n') throw new InvalidDataException("line"); return c; }
    private string? String(bool keep)
    {
        var text = keep ? new StringBuilder("\"") : null; bool escape = false;
        while (true)
        {
            int c = Required(); if (c < 32) throw new InvalidDataException("string");
            if (text != null) { text.Append((char)c); if (text.Length > 4096) text = null; }
            if (!escape && c == '"') break;
            if (!escape && c == '\\') escape = true; else escape = false;
        }
        if (text == null) return null;
        return Json.Read<string>(Encoding.UTF8.GetBytes(text.ToString()));
    }
    private JsonNode? Value(string? path, int depth)
    {
        if (depth > 64) throw new InvalidDataException("depth");
        Space(); int start = Required();
        if (start == '"') return String(path != null) is { } s ? JsonValue.Create(s) : null;
        if (start is '{' or '[')
        {
            bool obj = start == '{'; int end = obj ? '}' : ']';
            JsonObject? result = path != null && obj && Fields.ContainsKey(path) ? new() : null;
            Space(); if (reader.Peek() == end) { Read(); return result; }
            while (true)
            {
                string? key = null, next = null;
                if (obj)
                {
                    Space(); if (Required() != '"') throw new InvalidDataException("key");
                    key = String(result != null); Space(); if (Required() != ':') throw new InvalidDataException("colon");
                    if (key != null && result != null && Fields[path!].Contains(key)) next = path == "" ? key : path + "." + key;
                }
                var value = Value(next, depth + 1);
                if (key != null && next != null && result != null) result[key] = value;
                Space(); int separator = Required(); if (separator == end) break;
                if (separator != ',') throw new InvalidDataException("separator");
            }
            return result;
        }
        var scalar = path == null ? null : new StringBuilder().Append((char)start);
        while (reader.Peek() is var c && c >= 0 && c is not (',' or '}' or ']' or '\n' or '\r' or ' ' or '\t'))
        { Read(); if (scalar != null) { scalar.Append((char)c); if (scalar.Length > 64) throw new InvalidDataException("number"); } }
        return scalar == null ? null : JsonNode.Parse(scalar.ToString());
    }
    public IEnumerable<(JsonObject? Value, bool Invalid)> Rows()
    {
        while (reader.Peek() >= 0)
        {
            token.ThrowIfCancellationRequested(); Space(); if (reader.Peek() == '\n') { Read(); continue; }
            JsonObject? value = null; bool invalid = false, ended = false;
            try
            {
                value = Value("", 0) as JsonObject; Space();
                if (reader.Peek() is not ('\n' or -1)) throw new InvalidDataException("tail");
                if (reader.Peek() == '\n') Read();
            }
            catch (EndOfStreamException) { ended = true; }
            catch (Exception e) when (e is InvalidDataException or System.Text.Json.JsonException)
            { invalid = true; if (e is not InvalidDataException { Message: "line" }) while (reader.Peek() >= 0 && Read() != '\n') { } }
            if (ended) yield break; // Writers can leave a partially written last row.
            yield return (value, invalid);
        }
    }
}
