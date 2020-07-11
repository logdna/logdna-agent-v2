library 'magic-butler-catalogue'
def PROJECT_NAME = 'logdna-agent-v2'

pipeline {
    agent {
        table 'rust'
    }
    options {
        timestamps()
        ansiColor 'xterm'
    }
    stages {
        stage('Test') {
            steps {
                sh 'make test'
            }
        }
        stage('Clean') {
            steps {
                sh 'make clean'
            }
        }
        stage('Build') {
            steps {
                sh 'RELEASE=1 make build'
            }
        }
    }
}
