from flask import Flask, request

app = Flask(__name__)

@app.route('/logs/agent', methods=['POST'])
def index():
    print(request.__dict__)
    return ''

if __name__ == '__main__':
    app.run(host='0.0.0.0', port=80)
