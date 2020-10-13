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

  getExchanges() {
    const url = `${API_URL}/exchanges`;
    return client.request({ url, params: {}, method: 'get' }).then(data => data.data);
  },

  createApiKey(params) {
    const url = `${API_URL}/api_keys`;
    const ApiKeyParams = {
      exchange_id: params.exchangeId,
      key: params.key,
      secret: params.secret,
      german_trading_agreement: params.germanAgreement
    }
    return client.request({ url, data: { api_key: ApiKeyParams }, method: 'post' }).then(data => data.data);
  },

  createBot(params) {
    const url = `${API_URL}/bots`;
    const botParams= {
      bot_type: params.botType,
      exchange_id: params.exchangeId,
      type: params.type,
      order_type: params.order_type,
      price: params.price,
      percentage: params.percentage,
      currency: params.currency,
      interval: params.interval,
    }
    return client.request({ url, data: { bot: botParams }, method: 'post' }).then(data => data.data);
  },

  updateBot(params) {
    const url = `${API_URL}/bots/${params.id}`;
    const botParams= {
      order_type: params.order_type,
      price: params.price,
      percentage: params.percentage,
      interval: params.interval,
    }

    return client.request({ url, data: { bot: botParams }, method: 'put' }).then(data => data.data);
  },

  getBots() {
    const url = `${API_URL}/bots`;
    return client.request({ url, params: {}, method: 'get' }).then(data => data.data);
  },

  getBot(id) {
    const url = `${API_URL}/bots/${id}`;
    return client.request({ url, params: {}, method: 'get' }).then(data => data.data);
  },

  startBot(botId) {
    const url = `${API_URL}/bots/${botId}/start`;
    return client.request({ url, params: {}, method: 'post' }).then(data => data.data);
  },

  stopBot(botId) {
    const url = `${API_URL}/bots/${botId}/stop`;
    return client.request({ url, params: {}, method: 'post' }).then(data => data.data);
  },

  removeBot(botId) {
    const url = `${API_URL}/bots/${botId}`;
    return client.request({ url, params: {}, method: 'delete' }).then(data => data.data);
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
  }
};

export default API;
