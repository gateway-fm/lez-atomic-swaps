#ifndef LEZ_ROLE_SIGNER_H
#define LEZ_ROLE_SIGNER_H

// The role's LEZ signer, hosted by Basecamp: the desk asks Basecamp's lez_core
// (the LEZ wallet as a Logos module) to open the shared wallet, import this
// role's key and serve the role's signer socket, which the role Node's
// sidecars ask for the witness of every LEZ transaction they plan. A desk
// without the configuration below starts no signer.
//
//   LEZ_WALLET_DIR                 the wallet's storage directory (shared by both desks)
//   LEZ_WALLET_SEQUENCER_URL       the LEZ sequencer the wallet talks to
//   LEZ_<ROLE>_SIGNER_SOCKET       the role's signer socket
//   LEZ_<ROLE>_SIGNER_IDENTITY     the role identity's directory (identity.json, lez-signer.key)
//
// It writes one status line to <socket>.status so the stack can tell a desk
// that started its signer from one that did not.

#include <QDir>
#include <QFile>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QRandomGenerator>
#include <QString>

namespace lez_role_signer {

template <typename LezCore>
void start(LezCore& wallet, const QString& role)
{
    const QString prefix = QStringLiteral("LEZ_") + role.toUpper() + QStringLiteral("_SIGNER_");
    const QString socket = qEnvironmentVariable((prefix + QStringLiteral("SOCKET")).toUtf8().constData());
    if (socket.isEmpty()) return;
    QFile status(socket + QStringLiteral(".status"));
    const auto report = [&status](const QString& line) {
        if (status.open(QIODevice::WriteOnly | QIODevice::Truncate)) {
            status.write(line.toUtf8() + "\n");
            status.close();
        }
    };

    const QString directory = qEnvironmentVariable("LEZ_WALLET_DIR");
    const QString sequencer = qEnvironmentVariable("LEZ_WALLET_SEQUENCER_URL");
    const QString identity = qEnvironmentVariable((prefix + QStringLiteral("IDENTITY")).toUtf8().constData());
    QFile identityFile(identity + QStringLiteral("/identity.json"));
    QString account;
    if (identityFile.open(QIODevice::ReadOnly)) {
        account = QJsonDocument::fromJson(identityFile.readAll()).object().value(QStringLiteral("account_id_hex")).toString().toLower();
        identityFile.close();
    }
    QFile key(identity + QStringLiteral("/lez-signer.key"));
    if (directory.isEmpty() || sequencer.isEmpty() || account.size() != 64 || !key.open(QIODevice::ReadOnly)) {
        report(QStringLiteral("not started: incomplete signer configuration"));
        return;
    }
    const QString keyHex = QString::fromUtf8(key.readAll()).trimmed();
    key.close();

    QDir().mkpath(directory);
    const QString config = directory + QStringLiteral("/wallet_config.json");
    QFile configFile(config);
    if (configFile.open(QIODevice::WriteOnly | QIODevice::Truncate)) {
        const QJsonObject connection{{QStringLiteral("sequencer_addr"), sequencer},
                                     {QStringLiteral("basic_auth"), QJsonValue()}};
        configFile.write(QJsonDocument(QJsonObject{
            {QStringLiteral("sequencers"), QJsonArray{connection}},
            {QStringLiteral("seq_poll_timeout"), QStringLiteral("12s")},
            {QStringLiteral("seq_tx_poll_max_blocks"), 5},
            {QStringLiteral("seq_poll_max_retries"), 5},
            {QStringLiteral("seq_block_poll_max_amount"), 100}}).toJson(QJsonDocument::Compact));
        configFile.close();
    }
    const QString password = QString::number(QRandomGenerator::system()->generate64(), 16);
    const int opened = wallet.ensure_wallet(config, directory + QStringLiteral("/storage.json"),
                                            directory + QStringLiteral("/statistics.json"), password);
    const int imported = opened == 0 ? wallet.import_public_account(keyHex) : -1;
    const int serving = imported == 0 ? wallet.serve_signer(socket, account) : -1;
    report(serving == 0 ? QStringLiteral("serving ") + account
                        : QStringLiteral("not started: ensure_wallet %1, import %2, serve %3")
                              .arg(opened).arg(imported).arg(serving));
}

} // namespace lez_role_signer

#endif
