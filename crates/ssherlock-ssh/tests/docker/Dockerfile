# OpenSSH test server for ssherlock-ssh's integration tests. Never expose it beyond localhost:
# its user, password and keys are public (see ../fixtures/README.md).
FROM alpine:3.22

# The PAM build of sshd, because keyboard-interactive needs PAM on Linux.
RUN apk add --no-cache openssh-server-pam linux-pam \
    && ssh-keygen -A \
    && adduser -D -s /bin/sh tester \
    && echo 'tester:tester-password' | chpasswd

COPY docker/sshd_config /etc/ssh/sshd_config
COPY fixtures/id_ed25519.pub fixtures/id_ecdsa.pub fixtures/id_rsa.pub fixtures/id_ed25519_encrypted.pub /tmp/keys/
RUN mkdir -p /home/tester/.ssh \
    && cat /tmp/keys/*.pub > /home/tester/.ssh/authorized_keys \
    && rm -r /tmp/keys \
    && chown -R tester:tester /home/tester/.ssh \
    && chmod 700 /home/tester/.ssh \
    && chmod 600 /home/tester/.ssh/authorized_keys

EXPOSE 22
CMD ["/usr/sbin/sshd.pam", "-D", "-e"]
