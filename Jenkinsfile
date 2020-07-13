library 'magic-butler-catalogue'
def PROJECT_NAME = 'logdna-agent-v2'

pipeline {
    agent any

    options {
        timestamps()
        ansiColor 'xterm'
    }
    stages {
        stage('Build') {
            parallel {
                stage('Build Test Dependencies') {
                    steps {
                        sh 'make test-deps'
                    }
                }
                stage('Build Agent') {
                    steps {
                        sh 'make build'
                    }
                }
            }
        }
        stage('Test') {
            steps {
                sh 'make test'
            }
        }
        stage('Build Image') {
            steps {
                sh 'make build-image'
            }
        }
        stage('Deploy to Dockerhub') {
            steps {
                sh 'make publish'
            }
        }
    }
}
