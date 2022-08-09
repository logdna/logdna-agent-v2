library 'magic-butler-catalogue'
def PROJECT_NAME = 'logdna-agent-v2'
def TRIGGER_PATTERN = '.*@logdnabot.*'
def publishGCRImage = false
def publishDockerhubICRImages = false

pipeline {
    agent {
        node {
            label "rust-x86_64"
            customWorkspace("/tmp/workspace/${env.BUILD_TAG}")
        }
    }
    options {
        timeout time: 8, unit: 'HOURS'
        timestamps()
        ansiColor 'xterm'
    }
    triggers {
        issueCommentTrigger(TRIGGER_PATTERN)
        parameterizedCron(
            env.BRANCH_NAME ==~ /\d\.\d/ ? 'H 8 * * 1 % PUBLISH_GCR_IMAGE=true;PUBLISH_ICR_IMAGE=true' : ''
        )
    }
    environment {
        RUST_IMAGE_REPO = 'us.gcr.io/logdna-k8s/rust'
        RUST_IMAGE_TAG = 'bullseye-1-stable'
        SCCACHE_BUCKET = 'logdna-sccache-us-west-2'
        SCCACHE_REGION = 'us-west-2'
        CARGO_INCREMENTAL = 'false'
        DOCKER_BUILDKIT = '1'
    }
    parameters {
        booleanParam(name: 'PUBLISH_GCR_IMAGE', description: 'Publish docker image to Google Container Registry (GCR)', defaultValue: false)
        booleanParam(name: 'PUBLISH_ICR_IMAGE', description: 'Publish docker image to IBM Container Registry (ICR) and Dockerhub', defaultValue: false)
        booleanParam(name: 'PUBLISH_BINARIES', description: 'Publish executable binaries to S3 bucket s3://logdna-agent-build-bin', defaultValue: false)
        string(name: 'RUST_IMAGE_SUFFIX', description: 'Build image tag suffix', defaultValue: "")
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
        stage('Init QEMU') {
            steps {
                sh "make init-qemu"
            }
        }
        stage('Vendor') {
            steps {
                sh """
                    mkdir -p .cargo || /bin/true
                    make vendor
                """
            }
        }
        stage('Lint and Unit Test') {
            parallel {
                stage('Lint'){
                    steps {
                        withCredentials([[
                                           $class: 'AmazonWebServicesCredentialsBinding',
                                           credentialsId: 'aws',
                                           accessKeyVariable: 'AWS_ACCESS_KEY_ID',
                                           secretKeyVariable: 'AWS_SECRET_ACCESS_KEY'
                                         ]]){
                            sh """
                              make lint
                            """
                        }
                    }
                }
                stage('Unit Tests'){
                    steps {
                        withCredentials([[
                                           $class: 'AmazonWebServicesCredentialsBinding',
                                           credentialsId: 'aws',
                                           accessKeyVariable: 'AWS_ACCESS_KEY_ID',
                                           secretKeyVariable: 'AWS_SECRET_ACCESS_KEY'
                                         ]]){
                            sh """
                              make -j2 test
                            """
                        }
                    }
                }
            }
        }
        stage('Test') {
            environment {
                CREDS_FILE = credentials('pipeline-e2e-creds')
                LOGDNA_HOST = "logs.use.stage.logdna.net"
            }
            parallel {
                stage('Integration Tests'){
                    steps {
                        script {
                            def creds = readJSON file: CREDS_FILE
                            // Assumes the pipeline-e2e-creds format remains the same. Chase
                            // refer to the e2e tests's README's authorization docs for the
                            // current structure
                            LOGDNA_INGESTION_KEY = creds["packet-stage"]["account"]["ingestionkey"]
                            TEST_THREADS = sh (script: 'threads=$(echo $(nproc)/4 | bc); echo $(( threads > 1 ? threads: 1))', returnStdout: true).trim()
                        }
                        withCredentials([[
                                           $class: 'AmazonWebServicesCredentialsBinding',
                                           credentialsId: 'aws',
                                           accessKeyVariable: 'AWS_ACCESS_KEY_ID',
                                           secretKeyVariable: 'AWS_SECRET_ACCESS_KEY'
                                         ]]){
                            sh """
                              TEST_THREADS="${TEST_THREADS}" make integration-test LOGDNA_INGESTION_KEY=${LOGDNA_INGESTION_KEY}
                            """
                        }
                    }
                }
                stage('Run K8s Integration Tests') {
                    steps {
                        withCredentials([[
                                            $class: 'AmazonWebServicesCredentialsBinding',
                                            credentialsId: 'aws',
                                            accessKeyVariable: 'AWS_ACCESS_KEY_ID',
                                            secretKeyVariable: 'AWS_SECRET_ACCESS_KEY'
                                            ]]) {
                            sh """
                                echo "[default]" > ${WORKSPACE}/.aws_creds_k8s-test
                                echo "AWS_ACCESS_KEY_ID=${AWS_ACCESS_KEY_ID}" >> ${WORKSPACE}/.aws_creds_k8s-test
                                echo "AWS_SECRET_ACCESS_KEY=${AWS_SECRET_ACCESS_KEY}" >> ${WORKSPACE}/.aws_creds_k8s-test
                                make k8s-test AWS_SHARED_CREDENTIALS_FILE=${WORKSPACE}/.aws_creds_k8s-test
                            """
                        }
                    }
                }
            }
            post {
                always {
                    sh "make clean"
                    sh "rm -f ${WORKSPACE}/.aws_creds_k8s-test"
                }
            }
        }
        stage('Build Release Binaries') {
            environment {
                CREDS_FILE = credentials('pipeline-e2e-creds')
                LOGDNA_HOST = "logs.use.stage.logdna.net"
            }
            parallel {
                stage('Build Release Image x86_64') {
                    steps {
                        sh "make init-qemu"
                        withCredentials([[
                            $class: 'AmazonWebServicesCredentialsBinding',
                            credentialsId: 'aws',
                            accessKeyVariable: 'AWS_ACCESS_KEY_ID',
                            secretKeyVariable: 'AWS_SECRET_ACCESS_KEY'
                        ]]){
                            sh """
                                echo "[default]" > ${WORKSPACE}/.aws_creds_x86_64
                                echo "AWS_ACCESS_KEY_ID=${AWS_ACCESS_KEY_ID}" >> ${WORKSPACE}/.aws_creds_x86_64
                                echo "AWS_SECRET_ACCESS_KEY=${AWS_SECRET_ACCESS_KEY}" >> ${WORKSPACE}/.aws_creds_x86_64
                                ARCH=x86_64 make build-image AWS_SHARED_CREDENTIALS_FILE=${WORKSPACE}/.aws_creds_x86_64
                            """
                        }
                    }
                    post {
                        always {
                            sh "rm ${WORKSPACE}/.aws_creds_x86_64"
                        }
                    }
                }
                stage('Build Release Image aarch64') {
                    steps {
                        withCredentials([[
                            $class: 'AmazonWebServicesCredentialsBinding',
                            credentialsId: 'aws',
                            accessKeyVariable: 'AWS_ACCESS_KEY_ID',
                            secretKeyVariable: 'AWS_SECRET_ACCESS_KEY'
                        ]]){
                            sh """
                                echo "[default]" > ${WORKSPACE}/.aws_creds_aarch64
                                echo "AWS_ACCESS_KEY_ID=${AWS_ACCESS_KEY_ID}" >> ${WORKSPACE}/.aws_creds_aarch64
                                echo "AWS_SECRET_ACCESS_KEY=${AWS_SECRET_ACCESS_KEY}" >> ${WORKSPACE}/.aws_creds_aarch64
                                ARCH=aarch64 make build-image AWS_SHARED_CREDENTIALS_FILE=${WORKSPACE}/.aws_creds_aarch64
                            """
                        }
                    }
                    post {
                        always {
                            sh "rm ${WORKSPACE}/.aws_creds_aarch64"
                        }
                    }
                }
                stage('Build static release binary x86_64') {
                    steps {
                        withCredentials([[
                            $class: 'AmazonWebServicesCredentialsBinding',
                            credentialsId: 'aws',
                            accessKeyVariable: 'AWS_ACCESS_KEY_ID',
                            secretKeyVariable: 'AWS_SECRET_ACCESS_KEY'
                        ]]) {
                            sh '''
                                echo "[default]" > ${WORKSPACE}/.aws_creds_static_x86_64
                                echo "AWS_ACCESS_KEY_ID=${AWS_ACCESS_KEY_ID}" >> ${WORKSPACE}/.aws_creds_static_x86_64
                                echo "AWS_SECRET_ACCESS_KEY=${AWS_SECRET_ACCESS_KEY}" >> ${WORKSPACE}/.aws_creds_static_x86_64
                                ARCH=x86_64 STATIC=1 FEATURES= make build-release AWS_SHARED_CREDENTIALS_FILE=${WORKSPACE}/.aws_creds_static_x86_64
                                rm ${WORKSPACE}/.aws_creds_static_x86_64
                            '''
                        }
                    }
                }
                stage('Build static release binary aarch64') {
                    steps {
                        withCredentials([[
                            $class: 'AmazonWebServicesCredentialsBinding',
                            credentialsId: 'aws',
                            accessKeyVariable: 'AWS_ACCESS_KEY_ID',
                            secretKeyVariable: 'AWS_SECRET_ACCESS_KEY'
                        ]]) {
                            sh '''
                                echo "[default]" > ${WORKSPACE}/.aws_creds_static_aarch64
                                echo "AWS_ACCESS_KEY_ID=${AWS_ACCESS_KEY_ID}" >> ${WORKSPACE}/.aws_creds_static_aarch64
                                echo "AWS_SECRET_ACCESS_KEY=${AWS_SECRET_ACCESS_KEY}" >> ${WORKSPACE}/.aws_creds_static_aarch64
                                ARCH=aarch64 STATIC=1 FEATURES= make build-release AWS_SHARED_CREDENTIALS_FILE=${WORKSPACE}/.aws_creds_static_aarch64
                                rm ${WORKSPACE}/.aws_creds_static_aarch64
                            '''
                        }
                    }
                }
                stage('Build windows release binary x86_64') {
                    steps {
                        withCredentials([[
                            $class: 'AmazonWebServicesCredentialsBinding',
                            credentialsId: 'aws',
                            accessKeyVariable: 'AWS_ACCESS_KEY_ID',
                            secretKeyVariable: 'AWS_SECRET_ACCESS_KEY'
                        ]]) {
                            sh '''
                                echo "[default]" > ${WORKSPACE}/.aws_creds_win_static_x86_64
                                echo "AWS_ACCESS_KEY_ID=${AWS_ACCESS_KEY_ID}" >> ${WORKSPACE}/.aws_creds_win_static_x86_64
                                echo "AWS_SECRET_ACCESS_KEY=${AWS_SECRET_ACCESS_KEY}" >> ${WORKSPACE}/.aws_creds_win_static_x86_64
                                ARCH=x86_64 WINDOWS=1 FEATURES=windows_service make build-release AWS_SHARED_CREDENTIALS_FILE=${WORKSPACE}/.aws_creds_win_static_x86_64
                                rm ${WORKSPACE}/.aws_creds_win_static_x86_64
                            '''
                        }
                    }
                }
            }
        }
        stage('Check Publish Images') {
            stages {
                stage('Scanning Images') {
                    steps {
                        sh 'ARCH=x86_64 make sysdig_secure_images'
                        sysdig engineCredentialsId: 'sysdig-secure-api-token', name: 'sysdig_secure_images', inlineScanning: true
                        sh 'ARCH=aarch64 make sysdig_secure_images'
                        sysdig engineCredentialsId: 'sysdig-secure-api-token', name: 'sysdig_secure_images', inlineScanning: true
                    }
                }
                stage('Publish static binary') {
                    when {
                        anyOf {
                            branch pattern: "\\d\\.\\d.*", comparator: "REGEXP"
                            environment name: 'PUBLISH_BINARIES', value: 'true'
                        }
                    }
                    steps {
                        withCredentials([[
                            $class: 'AmazonWebServicesCredentialsBinding',
                            credentialsId: 'aws',
                            accessKeyVariable: 'AWS_ACCESS_KEY_ID',
                            secretKeyVariable: 'AWS_SECRET_ACCESS_KEY'
                        ]]) {
                            sh '''
                                echo "[default]" > ${WORKSPACE}/.aws_creds_static
                                echo "AWS_ACCESS_KEY_ID=${AWS_ACCESS_KEY_ID}" >> ${WORKSPACE}/.aws_creds_static
                                echo "AWS_SECRET_ACCESS_KEY=${AWS_SECRET_ACCESS_KEY}" >> ${WORKSPACE}/.aws_creds_static
                                STATIC=1 make publish-s3-binary
                                WINDOWS=1 make publish-s3-binary
                                WINDOWS=1 make msi-release
                                WINDOWS=1 make publish-s3-binary-signed-release
                                ARCH=x86_64 STATIC=1 make publish-s3-binary AWS_SHARED_CREDENTIALS_FILE=${WORKSPACE}/.aws_creds_static
                                ARCH=aarch64 STATIC=1 make publish-s3-binary AWS_SHARED_CREDENTIALS_FILE=${WORKSPACE}/.aws_creds_static
                                rm ${WORKSPACE}/.aws_creds_static
                            '''
                        }
                    }
                }
                stage('Publish GCR images') {
                    when {
                        environment name: 'PUBLISH_GCR_IMAGE', value: 'true'
                    }
                    steps {
                        // Publish to gcr, jenkins is logged into gcr globally
                        sh 'ARCH=x86_64 make publish-image-gcr'
                        sh 'ARCH=aarch64 make publish-image-gcr'
                        sh 'make publish-image-multi-gcr'
                    }
                }
                stage('Publish Dockerhub and ICR images') {
                    when {
                        environment name: 'PUBLISH_ICR_IMAGE', value: 'true'
                    }
                    steps {
                        script {
                            // Login and publish to dockerhub
                            docker.withRegistry(
                                'https://index.docker.io/v1/',
                                'dockerhub-username-password'
                            ) {
                                sh 'ARCH=x86_64 make publish-image-docker'
                                sh 'ARCH=aarch64 make publish-image-docker'
                                sh 'make publish-image-multi-docker'
                            }
                            // Login and publish to ibm
                            docker.withRegistry(
                                'https://icr.io',
                                'icr-iam-username-password'
                            ) {
                                sh 'ARCH=x86_64 make publish-image-ibm'
                                sh 'ARCH=aarch64 make publish-image-ibm'
                                sh 'make publish-image-multi-ibm'
                            }
                        }
                    }
                }
            }
            post {
                always {
                    sh 'ARCH=x86_64 make clean-all'
                    sh 'ARCH=aarch64 make clean-all'
                }
            }
        }
    }
}
