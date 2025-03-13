import React from 'react'
import I18n from 'i18n-js'
import { TradingTransactions } from './BotDetails/TradingTransactions';
import { Logs } from './BotDetails/Logs';
import { Info } from './BotDetails/Info';
import { isNotEmpty, isEmpty } from '../utils/array';
import { WithdrawalTransactions } from "./BotDetails/WithdrawalTransactions";
import { WebhookTransactions } from "./BotDetails/WebhookTransactions";

export const BotDetails = ({ bot }) => {
  const statisticsActive = isNotEmpty(bot.transactions) || bot.bot_type == "webhook"
  const logActive = isNotEmpty(bot.logs) && !statisticsActive
  const infoActive = isEmpty(bot.transactions) && isEmpty(bot.logs) && bot.bot_type != "webhook"

  const tabs = [
    {
      label: I18n.t('bots.details.stats.title'),
      active: statisticsActive,
      visible: statisticsActive,
      id: 'stats-tab',
      tabpanelId: 'statistics',
    },
    {
      label: I18n.t('bots.details.log.title'),
      active: logActive,
      visible: isNotEmpty(bot.logs),
      id: 'log-tab',
      tabpanelId: 'log',
    },
    {
      label: I18n.t('bots.details.info.title'),
      active: infoActive,
      visible: true,
      id: 'info-tab',
      tabpanelId: 'info',
    }
  ];

  const buildTab = ({ label, active, id, tabpanelId }, index) => (
    <div className="nav-item" key={index}>
      <a
        className={`nav-link ${active ? 'active' : ''}`}
        id={id}
        data-toggle="tab"
        href={`#${tabpanelId}`}
        role="tab"
        aria-controls={tabpanelId}
        aria-selected={active}
      >
        {label}
      </a>
    </div>
  )

  return (
    <div className="db-bots__item db-bots__item--data">
      <div className="db-bots__tabs" role="tablist">
        {tabs.filter(e => e.visible).map(buildTab)}
      </div>
      { bot.bot_type === 'trading' &&
        <TradingTransactions bot={bot} active={statisticsActive}/>
      }
      { bot.bot_type === 'withdrawal' &&
        <WithdrawalTransactions bot={bot} active={statisticsActive}/>
      }
      { bot.bot_type === 'webhook' &&
        <WebhookTransactions bot={bot} active={statisticsActive}/>
      }
      <Logs bot={bot} active={logActive}/>
      <Info bot={bot} active={infoActive} />
    </div>
  )
}
