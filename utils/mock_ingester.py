from flask import Flask, request
# 
# To tell agent to use this mock ingester:
#
#  export LOGDNA_HOST=localhost:8080
#  export LOGDNA_USE_SSL=false
#
app = Flask(__name__)

@app.route('/logs/agent', methods=['GET', 'POST'])
def index():
    print(request.__dict__)
    return ''

if __name__ == '__main__':
    app.run(host='0.0.0.0', port=7080)
