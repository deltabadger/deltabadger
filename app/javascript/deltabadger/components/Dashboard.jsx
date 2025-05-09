import React, {useEffect, useState} from 'react'
import { connect } from 'react-redux';
import I18n from 'i18n-js'
import { BotForm } from './BotForm'
import { BotDetails } from './BotDetails'
import { TradingBot } from './TradingBot'
import {
  startBot,
} from '../bot_actions'
import API from "../lib/API";
import { WithdrawalBot } from "./WithdrawalBot";
import { WebhookBot } from "./WebhookBot";
import { Spinner } from './Spinner';

let apiKeyTimeout;

const DashboardTemplate = ({
  isBasic,
  isPro,
  isLegendary,
  errors = {},
  startBot,
}) => {
  const [exchanges, setExchanges] = useState([]);
  const [step, setStep] = useState(0);
  const [isLoading, setIsLoading] = useState(false);
  const [selectedBotId, setSelectedBotId] = useState(null);
  const [isCreating, setIsCreating] = useState(false);
  const [selectedBot, setSelectedBot] = useState(null);

  const fetchExchanges = (type) => {
    API.getExchanges(type).then(data => setExchanges(data.data))
  }

  useEffect(() => {
    fetchExchanges('trading')
  }, [])

  useEffect(() => {
    const path = window.location.pathname;
    const botIdMatch = path.match(/\/bots\/(\d+)/);
    const urlParams = new URLSearchParams(window.location.search);
    const createMode = urlParams.get('create') === 'true';

    if (createMode) {
      setIsCreating(true);
    } else if (botIdMatch) {
      const botId = parseInt(botIdMatch[1]);
      setSelectedBotId(botId);
    }
  }, []);

  useEffect(() => {
    if (selectedBotId) {
      setIsLoading(true);
      API.getBot(selectedBotId)
        .then(response => {
          setSelectedBot(response.data);
        })
        .finally(() => {
          setIsLoading(false);
        });
    } else {
      setSelectedBot(null);
    }
  }, [selectedBotId]);

  const handleFinishCreating = (id = null) => {
    if (id) {
      startBot(id);
      const url = `/${I18n.locale}/bots/${id}`;
      window.location.href = url;
    } else {
      window.location.href = `/${I18n.locale}/bots`;
    }
  };

  const renderBotDetail = () => {
    if (!selectedBot) {
      return (
        <div className="db-bots db-bots--main">
          <div className="db-bots__item d-flex db-add-more-bots">
            <div className="db-spinner-positioner">
              <Spinner />
            </div>
          </div>
        </div>
      );
    }

    const BotComponent = selectedBot.bot_type === 'trading' ? TradingBot :
                        selectedBot.bot_type === 'withdrawal' ? WithdrawalBot :
                        selectedBot.bot_type === 'webhook' ? WebhookBot : null;

    return (
      <>
        <div className="db-bots db-bots--single">
          <BotComponent
            showLimitOrders={isBasic || isPro || isLegendary}
            bot={selectedBot}
            open={true}
            errors={errors[selectedBot.id]}
            fetchExchanges={() => fetchExchanges(selectedBot.bot_type)}
            exchanges={exchanges}
            apiKeyTimeout={apiKeyTimeout}
          />
          <BotDetails bot={selectedBot} />
        </div>
      </>
    );
  };

  if (isLoading) {
    return (
      <div className="db-bots db-bots--main">
        <div className="db-bots__item d-flex db-add-more-bots">
          <div className="db-spinner-positioner">
            <Spinner />
          </div>
        </div>
      </div>
    );
  }

  if (selectedBotId) {
    return renderBotDetail();
  }

  if (isCreating) {
    return (
      <div className="db-bots db-bots--single">
        <BotForm
          isBasic={isBasic}
          isPro={isPro}
          isLegendary={isLegendary}
          open={true}
          currentBot={null}
          callbackAfterCreation={handleFinishCreating}
          callbackAfterOpening={() => {}}
          callbackAfterClosing={handleFinishCreating}
          exchanges={exchanges}
          fetchExchanges={fetchExchanges}
          apiKeyTimeout={apiKeyTimeout}
          step={step}
          setStep={setStep}
        />
      </div>
    );
  }

  return;
};

const mapStateToProps = (state) => ({
  errors: state.errors,
})

const mapDispatchToProps = ({
  startBot: startBot,
})

export const Dashboard = connect(mapStateToProps, mapDispatchToProps)(DashboardTemplate)
