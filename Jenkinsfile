library 'magic-butler-catalogue'
def PROJECT_NAME = 'logdna-agent-v2'
def TRIGGER_PATTERN = '.*@logdnabot.*'

pipeline {
    agent {
        node {
            label "rust-x86_64"
            customWorkspace("/tmp/workspace/${env.BUILD_TAG}")
        }
    }
    options {
        timestamps()
        ansiColor 'xterm'
    }
    triggers {
        issueCommentTrigger(TRIGGER_PATTERN)
        parameterizedCron(
            env.BRANCH_NAME ==~ /\d\.\d/ ? 'H 8 * * 1 % PUBLISH_IMAGE=true;AUDIT=false;TASK_NAME=image-vulnerability-update' : '' +
            env.BRANCH_NAME ==~ /\d\.\d/ ? 'H 12 * * 1 % AUDIT=true;TASK_NAME=audit' : ''
        )
    }
    environment {
        RUST_IMAGE_REPO = 'us.gcr.io/logdna-k8s/rust'
        RUST_IMAGE_TAG = 'buster-1-stable-x86_64'
        SCCACHE_BUCKET = 'logdna-sccache-us-west-2'
        SCCACHE_REGION = 'us-west-2'
        CARGO_INCREMENTAL = 'false'
    }
    parameters {
        booleanParam(name: 'PUBLISH_IMAGE', description: 'Publish docker images', defaultValue: false)
        booleanParam(name: 'AUDIT', description: 'Check for application vulnerabilities with cargo audit', defaultValue: true)
        choice(name: 'TASK_NAME', choices: ['n/a', 'audit', 'image-vulnerability-update'], description: 'The name of the task being handled in this build, if applicable')
    }
    stages {
        stage('Validate PR Source') {
          when {
            expression { env.CHANGE_FORK }
            not {
                triggeredBy 'issueCommentCause'
            }
          }
          steps {
            error("A maintainer needs to approve this PR for CI by commenting")
          }
        }
        stage('Pull Build Image') {
            steps {
                sh "docker pull ${RUST_IMAGE_REPO}:${RUST_IMAGE_TAG}"
            }
        }
        stage ("Lint"){
            environment {
                CREDS_FILE = credentials('pipeline-e2e-creds')
                LOGDNA_HOST = "logs.use.stage.logdna.net"
            }
            when {
                environment name: 'AUDIT', value: 'true'
            }
            steps {
                sh """
                    make lint-audit
                """
            }
        }
        stage ("Lint and Test"){
            environment {
                CREDS_FILE = credentials('pipeline-e2e-creds')
                LOGDNA_HOST = "logs.use.stage.logdna.net"
            }

            when {
              beforeAgent true
              not {
                triggeredBy 'ParameterizedTimerTriggerCause'
              }
            }
            steps {
                script {
                    def creds = readJSON file: CREDS_FILE
                    // Assumes the pipeline-e2e-creds format remains the same. Chase
                    // refer to the e2e tests's README's authorization docs for the
                    // current structure
                    LOGDNA_INGESTION_KEY = creds["packet-stage"]["account"]["ingestionkey"]
                }
                withCredentials([[
                    $class: 'AmazonWebServicesCredentialsBinding',
                    credentialsId: 'aws',
                    accessKeyVariable: 'AWS_ACCESS_KEY_ID',
                    secretKeyVariable: 'AWS_SECRET_ACCESS_KEY'
                ]]){
                    sh """
                        make test
                        make integration-test LOGDNA_INGESTION_KEY=${LOGDNA_INGESTION_KEY}
                    """
                }
            }
            post {
                success {
                    sh "make clean"
                }
            }
        }
        stage('Build Release Image') {
            steps {
                withCredentials([[
                    $class: 'AmazonWebServicesCredentialsBinding',
                    credentialsId: 'aws',
                    accessKeyVariable: 'AWS_ACCESS_KEY_ID',
                    secretKeyVariable: 'AWS_SECRET_ACCESS_KEY'
                ]]){
                    sh """
                        echo "[default]" > ${PWD}/.aws_creds
                        echo "AWS_ACCESS_KEY_ID=${AWS_ACCESS_KEY_ID}" >> ${PWD}/.aws_creds
                        echo "AWS_SECRET_ACCESS_KEY=${AWS_SECRET_ACCESS_KEY}" >> ${PWD}/.aws_creds
                        make build-image AWS_SHARED_CREDENTIALS_FILE=${PWD}/.aws_creds
                    """
                }
            }
            post {
                always {
                    sh "rm ${PWD}/.aws_creds"
                }
            }
        }
        stage('Check Publish Images') {
            when {
                branch pattern: "\\d\\.\\d.*", comparator: "REGEXP"
            }
            stages {
                stage('Scanning Images') {
                    steps {
                        sh 'make sysdig_secure_images'
                        sysdig engineCredentialsId: 'sysdig-secure-api-token', name: 'sysdig_secure_images', inlineScanning: true
                    }
                }
                stage('Publish Images') {
                    when {
                        environment name: 'PUBLISH_IMAGE', value: 'true'
                    }
                    steps {
                        // Publish to gcr, jenkins is logged into gcr globally
                        sh 'make publish-image-gcr'
                        script {
                            // Login and publish to dockerhub
                            docker.withRegistry(
                                'https://index.docker.io/v1/',
                                'dockerhub-username-password'
                            ) {
                                sh 'make publish-image-docker'
                            }
                            // Login and publish to ibm
                            docker.withRegistry(
                                'https://icr.io',
                                'icr-iam-username-password'
                            ) {
                                sh 'make publish-image-ibm'
                            }
                        }
                    }
                }
            }
            post {
                always {
                    sh 'make clean-all'
                }
            }
        }
    }
    post {
        success {
            script {
                if (params.TASK_NAME == 'image-vulnerability-update') {
                    //TODO: change channel to #ibm-mezmo-agent after testing
                    notifySlack(
                        currentBuild.currentResult,
                        [channel: '#proj-agent'],
                        "`${PROJECT_NAME}` ${params.TASK_NAME} build took ${currentBuild.durationString.replaceFirst(' and counting', '')}."
                    )
                }
            }
        }
        unsuccessful {
            script {
                if (params.TASK_NAME == 'audit' || params.TASK_NAME == 'image-vulnerability-update') {
                    notifySlack(
                        currentBuild.currentResult,
                        [channel: '#proj-agent'],
                        "`${PROJECT_NAME}` ${params.TASK_NAME} build took ${currentBuild.durationString.replaceFirst(' and counting', '')}."
                    )
                }
            }
        }
    }
}
