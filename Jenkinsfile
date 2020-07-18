library 'magic-butler-catalogue'
def PROJECT_NAME = 'logdna-agent-v2'

pipeline {
    agent any

    options {
        timestamps()
        ansiColor 'xterm'
    }
    stages {
        stage('Build & Test') {
            steps {
                sh 'make -f Makefile.docker test'
            }
        }
        stage('Clean') {
            steps {
                sh 'make clean'
            }
        }
        stage('Publish') {
            steps {
                sh 'make -f Makefile.docker publish-public'
            }
        }
    }
    post {
        always {
            sh 'make -f Makefile.docker clean'
        }
    }
}
