import urllib.request
import urllib.error
import json

def main():
    url = 'http://127.0.0.1:8080/v1/chat/completions'
    payload = {
        "model": "gemma-4-31B-it-Q8_0.gguf",
        "messages": [
            {"role": "user", "content": "Hello from probe script"}
        ],
        "max_new_tokens": 1,
        "temperature": 0.2,
        "stream": False
    }
    data = json.dumps(payload).encode('utf-8')
    req = urllib.request.Request(url, data=data, headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=120) as resp:
            print('HTTP', resp.getcode())
            print(resp.read().decode('utf-8'))
    except urllib.error.HTTPError as e:
        print('HTTP', e.code)
        try:
            print(e.read().decode('utf-8'))
        except Exception:
            pass
    except Exception as e:
        print('ERROR', str(e))

if __name__ == '__main__':
    main()
