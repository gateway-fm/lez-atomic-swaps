#include <QGuiApplication>
#include <QQmlContext>
#include <QQmlEngine>
#include <QQmlApplicationEngine>
#include <QTimer>
#include <QUrl>

#include "logos_host.h"

int main(int argc, char* argv[])
{
    qputenv("QT_QUICK_CONTROLS_STYLE", "Basic");

    QGuiApplication app(argc, argv);
    app.setApplicationName("lez-atomic-swap-preview");
    app.setOrganizationName("LEZ atomic swaps preview");

    LogosHost logosHost;

    QQmlApplicationEngine engine;
    engine.rootContext()->setContextProperty(QStringLiteral("logos"), &logosHost);
    engine.rootContext()->setContextProperty(
        QStringLiteral("sourceRoot"), QStringLiteral(LEZ_PREVIEW_SOURCE_ROOT));

    QObject::connect(&engine, &QQmlApplicationEngine::objectCreationFailed, &app,
        []() { QCoreApplication::exit(1); }, Qt::QueuedConnection);
    engine.loadFromModule("LezSwapPreview", "Preview");

    QTimer::singleShot(600, &logosHost, [&logosHost]() {
        logosHost.markReady(QStringLiteral("lez_atomic_swap_maker"));
    });
    QTimer::singleShot(900, &logosHost, [&logosHost]() {
        logosHost.markReady(QStringLiteral("lez_atomic_swap_taker"));
    });

    return app.exec();
}
