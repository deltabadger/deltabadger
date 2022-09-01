import axios from 'axios';

const API_URL = '/api';

const element = document.getElementsByName('csrf-token')[0];
const token = element && element.getAttribute('content');

const client = axios.create({
  headers: { 'X-CSRF-Token': token },
});

const API = {
  get(endpoint, params) {
    const url = `${API_URL}/${endpoint}`;
    return client.request({ url, params, method: 'get' }).then(data => data.data);
  },

  post(endpoint, params) {
    return client.post(`/${endpoint}`, params).then(data => data.data);
  },

  getExchanges(type) {
    const url = `${API_URL}/exchanges`;
    return client.request({ url, params: { type: type }, method: 'get' }).then(data => {
      console.log("DATA",data)
      return data.data
    });
  },

  createApiKey(params) {
    const url = `${API_URL}/api_keys`;
    const ApiKeyParams = {
      exchange_id: params.exchangeId,
      key: params.key,
      secret: params.secret,
      passphrase: params.passphrase,
      german_trading_agreement: params.germanAgreement,
      key_type: params.type
    }
    return client.request({ url, data: { api_key: ApiKeyParams }, method: 'post' }).then(data => data.data);
  },

  removeInvalidApiKeys(params) {
    const url = `${API_URL}/remove_invalid_keys`;

    return client.request({ url, data: { exchange_id: params.exchangeId }, method: 'post' });
  },

  getSmartIntervalsInfo(params) {
    const url = `${API_URL}/smart_intervals_info`;
    const botParams = {
      exchange_id: params.exchangeId || null,
      exchange_name: params.exchangeName || null,
      price: params.price,
      base: params.base,
      quote: params.quote,
      force_smart_intervals: params.forceSmartIntervals
    }

    return client.request({ url, params: botParams , method: 'get' }).then(data => {
      return data.data});
  },

  setShowSmartIntervalsInfo() {
    const url = `${API_URL}/set_show_smart_intervals_info`;

    return client.request({ url, params: {} , method: 'post' }).then(data => {
      return data.data});
  },

  createTradingBot(params) {
    const url = `${API_URL}/bots`;
    const botParams = {
      bot_type: params.botType,
      exchange_id: params.exchangeId,
      type: params.type,
      order_type: params.order_type,
      price: params.price,
      percentage: params.percentage,
      base: params.base,
      quote: params.quote,
      interval: params.interval,
      force_smart_intervals: params.forceSmartIntervals,
      smart_intervals_value: params.smartIntervalsValue,
      price_range_enabled: params.priceRangeEnabled,
      price_range: [params.priceRange.low, params.priceRange.high],
      use_subaccount: params.useSubaccount,
      selected_subaccount: params.selectedSubaccount
    }
    return client.request({ url, data: { bot: botParams }, method: 'post' }).then(data => data.data);
  },

  createWithdrawalBot(params) {
    const url = `${API_URL}/bots`;
    const botParams = {
      bot_type: params.botType,
      currency: params.currency,
      address: params.address,
      threshold: params.threshold,
      threshold_enabled: params.thresholdEnabled,
      interval: params.interval,
      interval_enabled: params.intervalEnabled,
      exchange_id: params.exchangeId
    }
    return client.request({ url, data: { bot: botParams }, method: 'post' }).then(data => data.data);
  },

  updateTradingBot(params) {
    const url = `${API_URL}/bots/${params.id}`;
    const botParams= {
      order_type: params.order_type,
      price: params.price,
      percentage: params.percentage,
      interval: params.interval,
      force_smart_intervals: params.forceSmartIntervals,
      smart_intervals_value: params.smartIntervalsValue,
      price_range_enabled: params.priceRangeEnabled,
      price_range: [params.priceRange.low, params.priceRange.high],
      use_subaccount: params.useSubaccount,
      selected_subaccount: params.selectedSubaccount
    }
    return client.request({ url, data: { bot: botParams }, method: 'put' }).then(data => data.data);
  },

  updateWithdrawalBot(params) {
    const url = `${API_URL}/bots/${params.id}`;
    const botParams= {
      threshold: params.threshold,
      threshold_enabled: params.thresholdEnabled,
      interval: params.interval,
      interval_enabled: params.intervalEnabled
    }

    return client.request({ url, data: { bot: botParams }, method: 'put' }).then(data => data.data);
  },

  continueBot(continueSchedule) {
    const url = `${API_URL}/bots/${params.id}/continue`;
    const botParams= {
      continue_schedule: continueSchedule
    }

    return client.request({ url, data: { bot: botParams }, method: 'post' }).then(data => data.data);
  },

  getBots(page) {
    const url = `${API_URL}/bots`;
    return client.request({ url, params: {page: page}, method: 'get' }).then(data => data.data);
  },

  getBot(id) {
    const url = `${API_URL}/bots/${id}`;
    return client.request({ url, params: {}, method: 'get' }).then(data => data.data);
  },

  startBot(params) {
    const url = `${API_URL}/bots/${params.id}/start`;
    const continueParams= {
      continue_schedule: params.continueParams.continueSchedule,
      price: params.continueParams.price
    }

    return client.request({ url, data: {continue_params: continueParams}, method: 'post' }).then(data => data.data);
  },

  stopBot(botId) {
    const url = `${API_URL}/bots/${botId}/stop`;
    return client.request({ url, params: {}, method: 'post' }).then(data => data.data);
  },

  removeBot(botId) {
    const url = `${API_URL}/bots/${botId}`;
    return client.request({ url, params: {}, method: 'delete' }).then(data => data.data);
  },

  fetchRestartParams(botId) {
    const url = `${API_URL}/bots/${botId}/restart_params`;
    return client.request({ url, params: {}, method: 'get' }).then(data => data.data)
  },

  getSubscription() {
    const url = `${API_URL}/subscriptions/check`;
    return client.request({ url, params: {}, method: 'get' }).then(data => data.data);
  },

  addSubscriber(email) {
    const url = `/newsletter/add_email`;
    return client.request({ url, data: { email }, method: 'post' }).then(data => data.data);
  },

  getChartData(botId) {
    const url = `${API_URL}/bots/${botId}/charts/portfolio_value_over_time`;
    return client.request({ url, params: {}, method: 'get' }).then(data => data.data);
  },

  checkFrequencyExceed(params) {
    const url = `${API_URL}/frequency_limit_exceeded`
    return client.request({url, params: params, method: 'get'}).then(data => data.data);
  },

  getSubaccounts(exchange_id) {
    const url = `${API_URL}/subaccounts`
    return client.request({url, params: {exchange_id: exchange_id}, method: 'get'}).then(data => data.data);
  },

  getWithdrawalMinimums(exchangeId, currency) {
    const params = {
      exchange_id: exchangeId,
      currency: currency
    }
    const url = `${API_URL}/withdrawal_minimums`
    return client.request({url, params: params, method: 'get'}).then(data => data.data);
  }
};

export default API;
