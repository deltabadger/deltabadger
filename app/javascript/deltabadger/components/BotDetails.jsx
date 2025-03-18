import React, { useState } from 'react'
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

  const [activeTab, setActiveTab] = useState(
    statisticsActive ? 'statistics' : 
    logActive ? 'log' : 
    'info'
  );

  const tabs = [
    {
      label: I18n.t('bots.details.stats.title'),
      active: activeTab === 'statistics',
      visible: statisticsActive,
      id: 'stats-tab',
      tabpanelId: 'statistics',
    },
    {
      label: I18n.t('bots.details.log.title'),
      active: activeTab === 'log',
      visible: isNotEmpty(bot.logs),
      id: 'log-tab',
      tabpanelId: 'log',
    },
    {
      label: I18n.t('bots.details.info.title'),
      active: activeTab === 'info',
      visible: true,
      id: 'info-tab',
      tabpanelId: 'info',
    }
  ];

  const handleTabClick = (tabId) => {
    setActiveTab(tabId);
  };

  const buildTab = ({ label, active, id, tabpanelId }, index) => (
    <button
      className={`nav-link ${active ? 'active' : ''}`}
      id={id}
      onClick={() => handleTabClick(tabpanelId)}
      role="tab"
      aria-controls={tabpanelId}
      aria-selected={active}
      key={index}
    >
      {label}
    </button>
  )

  return (
    <div className="db-bots__item db-bots__item--data">
      <div className="db-bots__tabs" role="tablist">
        {tabs.filter(e => e.visible).map(buildTab)}
      </div>
      { bot.bot_type === 'trading' &&
        <TradingTransactions bot={bot} active={activeTab === 'statistics'}/>
      }
      { bot.bot_type === 'withdrawal' &&
        <WithdrawalTransactions bot={bot} active={activeTab === 'statistics'}/>
      }
      { bot.bot_type === 'webhook' &&
        <WebhookTransactions bot={bot} active={activeTab === 'statistics'}/>
      }
      <Logs bot={bot} active={activeTab === 'log'}/>
      <Info bot={bot} active={activeTab === 'info'} />
    </div>
  )
}
