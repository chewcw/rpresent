#include <QGuiApplication>
#include <QQmlApplicationEngine>
#include <QQmlContext>
#include <QObject>
#include <QVariantMap>
#include <QJsonDocument>
#include <QJsonObject>
#include <QThread>
#include <QTextStream>
#include <QIODevice>
#include <QDebug>

class OverlayState : public QObject
{
    Q_OBJECT
    Q_PROPERTY(QVariantMap state   READ state   NOTIFY stateChanged)
    Q_PROPERTY(bool        visible READ visible NOTIFY visibilityChanged)

public:
    explicit OverlayState(QObject *parent = nullptr) : QObject(parent) {}

    QVariantMap state()   const { return m_state; }
    bool        visible() const { return m_visible; }

    Q_INVOKABLE void applyState(const QVariantMap &s)
    {
        m_state = s;
        emit stateChanged();
    }

    Q_INVOKABLE void applyVisibility(bool v)
    {
        m_visible = v;
        emit visibilityChanged();
    }

signals:
    void stateChanged();
    void visibilityChanged();
    void quitRequested();

private:
    QVariantMap m_state;
    bool        m_visible = false;
};

class StdinReader : public QThread
{
    Q_OBJECT
public:
    explicit StdinReader(OverlayState *state, QObject *parent = nullptr)
        : QThread(parent), m_state(state) {}

protected:
    void run() override
    {
        QTextStream in(stdin, QIODevice::ReadOnly);
        QString line;
        bool quit = false;
        while (in.readLineInto(&line)) {
            if (line.trimmed().isEmpty())
                continue;

            QJsonParseError err;
            const auto doc = QJsonDocument::fromJson(line.toUtf8(), &err);
            if (err.error != QJsonParseError::NoError) {
                qWarning() << "rpresent-qml-host: JSON parse error:" << err.errorString();
                continue;
            }

            const QString type = doc.object().value(QStringLiteral("type")).toString();

            if (type == QLatin1String("show")) {
                QMetaObject::invokeMethod(m_state, "applyVisibility",
                    Qt::QueuedConnection, Q_ARG(bool, true));
            } else if (type == QLatin1String("hide")) {
                QMetaObject::invokeMethod(m_state, "applyVisibility",
                    Qt::QueuedConnection, Q_ARG(bool, false));
            } else if (type == QLatin1String("redraw")) {
                const QVariantMap map = doc.object().toVariantMap();
                QMetaObject::invokeMethod(m_state, "applyState",
                    Qt::QueuedConnection, Q_ARG(QVariantMap, map));
                QMetaObject::invokeMethod(m_state, "applyVisibility",
                    Qt::QueuedConnection, Q_ARG(bool, true));
            } else if (type == QLatin1String("quit")) {
                quit = true;
                QMetaObject::invokeMethod(m_state, "quitRequested",
                    Qt::QueuedConnection);
                break;
            }
        }
        // Emit quit on EOF only if not already signalled by explicit quit message.
        if (!quit) {
            QMetaObject::invokeMethod(m_state, "quitRequested", Qt::QueuedConnection);
        }
    }

private:
    OverlayState *m_state;
};

int main(int argc, char *argv[])
{
    QGuiApplication app(argc, argv);

    OverlayState state;
    QQmlApplicationEngine engine;
    engine.rootContext()->setContextProperty(QStringLiteral("overlay"), &state);

    engine.load(QUrl(QStringLiteral("qrc:/qml/main.qml")));
    if (engine.rootObjects().isEmpty()) {
        qCritical() << "rpresent-qml-host: failed to load QML root";
        return 1;
    }

    QObject::connect(&state, &OverlayState::quitRequested,
                     &app,   &QGuiApplication::quit);

    StdinReader reader(&state);
    reader.start();

    const int ret = app.exec();
    reader.wait();
    return ret;
}

#include "main.moc"
