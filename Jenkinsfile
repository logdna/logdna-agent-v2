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
        parameterizedCron(env.BRANCH_NAME ==~ /\d\.\d/ ? '''
            H 8 * * 1 % PUBLISH_GCR_IMAGE=true;PUBLISH_ICR_IMAGE=true;AUDIT=false;TASK_NAME=image-vulnerability-update
            H 12 * * 1 % AUDIT=true;TASK_NAME=audit
            ''' : ''
        )
    }
    environment {
        RUST_IMAGE_REPO = 'us.gcr.io/logdna-k8s/rust'
        RUST_IMAGE_TAG = 'bullseye-1-stable'
        TOOLS_IMAGE_TAG = 'bullseye-1-stable'
        SCCACHE_BUCKET = 'logdna-sccache-us-west-2'
        SCCACHE_REGION = 'us-west-2'
        CARGO_INCREMENTAL = 'false'
        DOCKER_BUILDKIT = '1'
        REMOTE_BRANCH = "${env.BRANCH_NAME}"
    }
    parameters {
        booleanParam(name: 'PUBLISH_GCR_IMAGE', description: 'Publish docker image to Google Container Registry (GCR)', defaultValue: false)
        booleanParam(name: 'PUBLISH_ICR_IMAGE', description: 'Publish docker image to IBM Container Registry (ICR) and Dockerhub', defaultValue: false)
        booleanParam(name: 'PUBLISH_BINARIES', description: 'Publish executable binaries to S3 bucket s3://logdna-agent-build-bin', defaultValue: false)
        booleanParam(name: 'PUBLISH_INSTALLERS', description: 'Publish Choco installer', defaultValue: false)
        booleanParam(name: 'AUDIT', description: 'Check for application vulnerabilities with cargo audit', defaultValue: true)
        string(name: 'RUST_IMAGE_SUFFIX', description: 'Build image tag suffix', defaultValue: "")
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
        stage('Lint, Audit, and Unit Test') {
            parallel {
                stage('Lint') {
                    steps {
                        sh '''
                            make lint -o lint-audit
                        '''
                    }
                }
                stage('Audit') {
                    when {
                        environment name: 'AUDIT', value: 'true'
                    }
                    steps {
                        sh '''
                            make lint-audit
                        '''
                    }
                }
                stage('Mac OSX Unit Tests'){
                    when {
                        not {
                            triggeredBy 'ParameterizedTimerTriggerCause'
                        }
                    }
                    agent {
                        node {
                            label "osx-node"
                            customWorkspace("/tmp/workspace/${env.BUILD_TAG}")
                        }
                    }
                    steps {
                        sh """
                            source $HOME/.cargo/env
                            cargo make unit-tests
                        """
                    }
                    post {
                      always {
                        sh "cargo clean"
                        sh "rm -f ${WORKSPACE}/.aws_creds_mac_static_arm64"
                      }
                    }
                }
                stage('Unit Tests'){
                    when {
                        not {
                            triggeredBy 'ParameterizedTimerTriggerCause'
                        }
                    }
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
                stage('Code Coverage'){
                    steps {
                        withCredentials([[
                                           $class: 'AmazonWebServicesCredentialsBinding',
                                           credentialsId: 'aws',
                                           accessKeyVariable: 'AWS_ACCESS_KEY_ID',
                                           secretKeyVariable: 'AWS_SECRET_ACCESS_KEY'
                                         ]]){
                            sh """
                              make unit-code-coverage
                            """
                        }
                    }
                    post {
                        // Publish HTML code coverage report
                        success {
                            publishHTML (target: [
                                allowMissing: false,
                                alwaysLinkToLastBuild: true,
                                keepAll: true,
                                reportDir: "target/llvm-cov/html",
                                reportFiles: 'index.html',
                                reportName: 'Code Coverage Report',
                                reportTitles: 'Mezmo Agent Code Coverage'
                            ])
                        }
                    }
                }
            }
        }
        stage('Test') {
            when {
                not {
                    triggeredBy 'ParameterizedTimerTriggerCause'
                }
            }
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
                stage('Mac OSX Integration Tests'){
                    agent {
                        node {
                            label "osx-node"
                            customWorkspace("/tmp/workspace/${env.BUILD_TAG}")
                        }
                    }
                    environment {
                        CREDS_FILE = credentials('pipeline-e2e-creds')
                        LOGDNA_HOST = "logs.use.stage.logdna.net"
                    }
                    steps {
                        script {
                            def creds = readJSON file: CREDS_FILE
                            LOGDNA_INGESTION_KEY = creds["packet-stage"]["account"]["ingestionkey"]
                        }
                        sh """
                            source $HOME/.cargo/env
                            LOGDNA_INGESTION_KEY=${LOGDNA_INGESTION_KEY} LOGDNA_HOST=${LOGDNA_HOST} cargo make int-tests
                        """
                    }
                    post {
                      always {
                        sh "cargo clean"
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
                                ARCH=x86_64 make build-stress-test-image AWS_SHARED_CREDENTIALS_FILE=${WORKSPACE}/.aws_creds_x86_64
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
                                ARCH=aarch64 make build-stress-test-image AWS_SHARED_CREDENTIALS_FILE=${WORKSPACE}/.aws_creds_aarch64
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
                stage('Build Windows release binary x86_64') {
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
                stage('Build Mac OSX release binary X86_64') {
                    agent {
                        node {
                            label "osx-node"
                            customWorkspace("/tmp/workspace/${env.BUILD_TAG}/x86")
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
                                source $HOME/.cargo/env
                                source ~/.bash_profile
                                echo "[default]" > ${WORKSPACE}/.aws_creds_mac_static_x86_64
                                echo "AWS_ACCESS_KEY_ID=${AWS_ACCESS_KEY_ID}" >> ${WORKSPACE}/.aws_creds_mac_static_x86_64
                                echo "AWS_SECRET_ACCESS_KEY=${AWS_SECRET_ACCESS_KEY}" >> ${WORKSPACE}/.aws_creds_mac_static_x86_64
                                cargo build --release --target=x86_64-apple-darwin --target-dir x86-target
                            '''
                        }
                    }
                    post {
                      always {
                        sh "cargo clean"
                        sh "rm -f ${WORKSPACE}/.aws_creds_mac_static_x86_64"
                      }
                    }
                }
                stage('Build Mac OSX release binary ARM64') {
                    agent {
                        node {
                            label "osx-node"
                            customWorkspace("/tmp/workspace/${env.BUILD_TAG}/arm")
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
                                source $HOME/.cargo/env
                                source ~/.bash_profile
                                echo "[default]" > ${WORKSPACE}/.aws_creds_mac_static_arm64
                                echo "AWS_ACCESS_KEY_ID=${AWS_ACCESS_KEY_ID}" >> ${WORKSPACE}/.aws_creds_mac_static_arm64
                                echo "AWS_SECRET_ACCESS_KEY=${AWS_SECRET_ACCESS_KEY}" >> ${WORKSPACE}/.aws_creds_mac_static_arm64
                                cargo build --release --target-dir arm-target
                            '''
                        }
                    }
                    post {
                      always {
                        sh "cargo clean"
                        sh "rm -f ${WORKSPACE}/.aws_creds_mac_static_arm64"
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
                        script {
                            def imageNames = readFile('sysdig_secure_images').trim().split('\n')
                            for(img in imageNames){
                                echo "Scanning image ${img}"
                                sysdigImageScan engineCredentialsId: 'sysdig-secure-api-token', imageName: img
                            }
                        }
                        sh 'ARCH=aarch64 make sysdig_secure_images'
                        script {
                            def imageNames = readFile('sysdig_secure_images').trim().split('\n')
                            for(img in imageNames){
                                echo "Scanning image ${img}"
                                sysdigImageScan engineCredentialsId: 'sysdig-secure-api-token', imageName: img
                            }
                        }
                    }
                }
                stage('Publish Linux and Windows binaries to S3') {
                    when {
                        anyOf {
                            branch pattern: "\\d\\.\\d.*", comparator: "REGEXP"
                            environment name: 'PUBLISH_BINARIES', value: 'true'
                        }
                    }
                    environment {
                        CSC_PASS = credentials('chocolatey-api-token')
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
                                WINDOWS=1 make test-msi-release
                                WINDOWS=1 make publish-s3-binary-signed-release
                                WINDOWS=1 make choco-release
                                WINDOWS=1 make publish-s3-choco-release
                                ARCH=x86_64 STATIC=1 make publish-s3-binary AWS_SHARED_CREDENTIALS_FILE=${WORKSPACE}/.aws_creds_static
                                ARCH=aarch64 STATIC=1 make publish-s3-binary AWS_SHARED_CREDENTIALS_FILE=${WORKSPACE}/.aws_creds_static
                                rm ${WORKSPACE}/.aws_creds_static
                            '''
                        }
                    }
                }
                stage('Publish MAC binaries to S3') {
                    when {
                        anyOf {
                            branch pattern: "\\d\\.\\d.*", comparator: "REGEXP"
                            environment name: 'PUBLISH_BINARIES', value: 'true'
                        }
                    }
                    agent {
                        node {
                            label "osx-node"
                            customWorkspace("/tmp/workspace/${env.BUILD_TAG}")
                        }
                    }
                    steps {
                        withCredentials([[
                            $class: 'AmazonWebServicesCredentialsBinding',
                            credentialsId: 'aws',
                            accessKeyVariable: 'AWS_ACCESS_KEY_ID',
                            secretKeyVariable: 'AWS_SECRET_ACCESS_KEY'
                        ]]) {
                            sh """
                                source $HOME/.cargo/env
                                source ~/.bash_profile
                                echo "[default]" > ${WORKSPACE}/.aws_creds_mac_static_arm64
                                echo "AWS_ACCESS_KEY_ID=${AWS_ACCESS_KEY_ID}" >> ${WORKSPACE}/.aws_creds_mac_static_arm64
                                echo "AWS_SECRET_ACCESS_KEY=${AWS_SECRET_ACCESS_KEY}" >> ${WORKSPACE}/.aws_creds_mac_static_arm64
                                MACOS=1 make publish-s3-binary
                                rm ${WORKSPACE}/.aws_creds_mac_static_arm64
                            """
                        }
                    }
                }
                stage('Publish Installers') {
                    environment {
                        CHOCO_API_KEY = credentials('chocolatey-api-token')
                    }
                    when {
                        environment name: 'PUBLISH_INSTALLERS', value: 'true'
                    }
                    steps {
                        sh 'WINDOWS=1 make publish-choco-release'
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
                        sh 'ARCH=x86_64 make publish-stress-test-image-gcr'
                        sh 'ARCH=aarch64 make publish-stress-test-image-gcr'
                        sh 'make publish-stress-test-image-multi-gcr'
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
                                sh 'ARCH=x86_64 make publish-stress-test-image-docker'
                                sh 'ARCH=aarch64 make publish-stress-test-image-docker'
                                sh 'make publish-stress-test-image-multi-docker'
                            }
                            // Login and publish to ibm
                            docker.withRegistry(
                                'https://icr.io',
                                'icr-iam-username-password'
                            ) {
                                sh 'ARCH=x86_64 make publish-image-ibm'
                                sh 'ARCH=aarch64 make publish-image-ibm'
                                sh 'make publish-image-multi-ibm'
                                //sh 'ARCH=x86_64 make publish-stress-test-image-ibm'
                                //sh 'ARCH=aarch64 make publish-stress-test-image-ibm'
                                //sh 'make publish-stress-test-image-multi-ibm'
                            }
                        }
                    }
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
        always {
            sh 'ARCH=x86_64 make clean-all'
            sh 'ARCH=aarch64 make clean-all'
            cleanWs(deleteDirs: true,
                    // Uncomment the 'cleanWhenFailure: false,' line for debug
                    // cleanWhenFailure: false,
                    notFailBuild: true,
                    patterns: [[pattern: '.gitignore', type: 'INCLUDE'],
                                [pattern: '.propsfile', type: 'EXCLUDE']])
        }
    }
}
