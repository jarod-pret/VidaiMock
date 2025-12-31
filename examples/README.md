# VidaiMock Examples

This directory contains 20+ ready-to-use template examples for VidaiMock. You can use these templates by referencing them in your `providers.yaml` configuration.

## How to Use

1.  **Copy** any template from `templates/` to your `templates/` directory (or point your generic provider directly here).
2.  **Reference** the template in your `config/providers/my_provider.yaml` file:
    ```yaml
    request_mapping:
       # ...
    response_template: "templates/01_simple_echo.json.j2"
    ```

## Template Catalog

| File | Description | Features Used |
| :--- | :--- | :--- |
| `01_simple_echo.json.j2` | Basic API Response | `uuid`, `timestamp`, `json.messages` |
| `02_logic_control.json.j2` | Conditional Logic | `if/else`, string filters |
| `03_reflection.json.j2` | Request Inspection | `headers`, `query` access |
| `04_random_data_loop.json.j2` | Data Generation | `for` loop, `random_int`, `random_float` |
| `05_tool_calling.json.j2` | LLM Tool Calling | Simulating function arguments |
| `06_rate_limit_error.json.j2` | Error Simulation | Standard API error format |
| `07_openai_stream_chunk.json.j2`| Streaming (OpenAI) | `chunk` variable usage |
| `08_anthropic_message.json.j2` | Anthropic Format | Message API structure |
| `09_gemini_candidates.json.j2` | Gemini Format | Candidates structure |
| `10_rag_citations.json.j2` | RAG Simulation | Dynamic footnotes/citations |
| `11_security_fuzz.json.j2` | Security Testing | Large random string generation |
| `12_image_gen.json.j2` | Image Mocks | Placeholder image URLs |
| `13_maintenance.json.j2` | Static Responses | Simple static JSON |
| `14_complex_nested.json.j2` | Deep Nesting | Nested loops and objects |
| `15_html_page.html.j2` | HTML Content | Non-JSON responses |
| `16_data.csv.j2` | CSV Export | Comma-separated output |
| `17_math_solver.json.j2` | Hardcoded Logic | Simple deterministic responses |
| `18_soap_response.xml.j2` | XML / SOAP | Legacy protocol mocking |
| `19_graphql_response.json.j2` | GraphQL | GQL data structure |
| `20_empty.txt.j2` | 204 No Content | Empty body simulation |

## Provider Configurations (`examples/providers/`)

The `providers/` folder contains ready-to-run YAML configurations that use the templates above. To use them:

1.  **Copy** a file from `examples/providers/` to your main `config/providers/` directory.
2.  **Restart** the server (or it will auto-reload on next request if that feature is enabled).

| File | Matches Route | Helper Template Used |
| :--- | :--- | :--- |
| `01_echo.yaml` | `/examples/echo` | `01_simple_echo.json.j2` |
| `05_tools.yaml` | `/examples/tools` | `05_tool_calling.json.j2` |
| `07_streaming.yaml` | `/examples/openai/chat/completions` | `07_openai_stream_chunk.json.j2` |
| `10_rag.yaml` | `/examples/rag` | `10_rag_citations.json.j2` |

## Quick Start Configuration

To use the **Simple Echo** examplestr, copy `examples/providers/01_echo.yaml` to `config/providers/` and run:

```bash
curl -X POST http://localhost:3000/examples/echo -d '{"messages": [{"content": "Hello!"}]}'
```
