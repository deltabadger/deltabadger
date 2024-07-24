require 'sidekiq/web'
require 'telegram/bot'
require 'sidekiq/prometheus/exporter'

Rails.application.routes.draw do

  get 'sso', to: 'sso#sso'

  get 'errors/not_found'
  get 'errors/internal_server_error'
  match "/404", to: "errors#redirect_to_root", via: :all
  match "/422", to: "errors#unprocessable_entity", via: :all
  match "/500", to: "errors#internal_server_error", via: :all

  mount Sidekiq::Prometheus::Exporter => '/sidekiq-metrics'

  authenticate :user, lambda { |u| u.admin? } do
    mount Sidekiq::Web => '/sidekiq'
  end

  namespace :admin do
    resources :users, except: [:destroy]
    resources :affiliates, except: [:destroy] do
      get :wallet_csv, on: :collection
      get :accounting_csv, on: :collection
      post :mark_as_exported, on: :collection
      post :mark_as_paid, on: :collection
      put :get_fiat_commissions, on: :collection
    end
    resources :api_keys, except: [:edit, :update]
    resources :bots
    resources :conversion_rates
    resources :exchanges
    resources :transactions
    resources :subscriptions
    resources :subscription_plans
    resources :payments do
      get :csv, on: :collection
      get :csv_wire, on: :collection
      put '/confirm/:id', action: :confirm, as: :confirm, on: :collection
    end
    put '/change_setting_flag', to: 'settings#change_setting_flag'

    resources :vat_rates

    get :dashboard, to: 'dashboard#index'

    root to: "dashboard#index"
  end

  post '/newsletter/add_email', to: 'newsletter#add_email'
  namespace :api do
    get '/subscriptions/check', to: 'subscriptions#check'
    resources :api_keys, only: [:create]
    resources :exchanges, only: [:index]
    get :smart_intervals_info, to: 'bots#smart_intervals_info'
    get :subaccounts, to: 'bots#subaccounts'
    get :frequency_limit_exceeded, to: 'bots#frequency_limit_exceeded'
    get :withdrawal_minimums, to: 'bots#withdrawal_minimums'
    post :set_show_smart_intervals_info, to: 'bots#set_show_smart_intervals_info'
    post :remove_invalid_keys, to: 'api_keys#remove_invalid_keys'
    resources :bots, except: [:new, :edit] do
      post :stop, on: :member
      post :start, on: :member
      get :transactions_csv, to: 'transactions#csv'
      get 'charts/portfolio_value_over_time', to: 'charts#portfolio_value_over_time'
      get :restart_params
    end
    get '/webhook_bots_data', to: 'bots#webhook_bots_data'
  end

  scope "/(:lang)", lang: /#{I18n.available_locales.join("|")}/ do
    root to: 'home#index'

    devise_for :users, controllers: { sessions: 'users/sessions', passwords: 'users/passwords', confirmations: 'users/confirmations' }, skip: [:registrations]

    as :user do
      scope :users do
        get '/cancel', to: 'users/registrations#cancel', as: 'cancel_user_registration'
        get '/sign_up', to: 'users/registrations#new', as: 'new_user_registration'
        post '/', to: 'users/registrations#create', as: 'user_registration'
      end
    end

    resource :affiliate, path: 'referral-program', only: [:new, :create, :show] do
      get ':token/confirm_btc_address', action: 'confirm_btc_address', as: :confirm_btc_address
      patch :update_visible_info
      patch :update_btc_address
      patch :new, to: 'affiliates#new'
      patch :create, to: 'affiliates#create'
    end

    namespace :upgrade do
      get '/', action: :index
      post :btcpay_payment
      get :btcpay_payment_success
      post :btcpay_payment_ipn
      post :wire_transfer_payment
      post :zen_payment
      get :zen_payment_finished
      post :zen_payment_ipn
    end

    namespace :settings do
      get '/', action: :index
      patch :hide_welcome_banner
      patch :hide_news_banner
      patch :hide_referral_banner
      patch :update_password
      patch :update_email
      patch :update_name
      post :enable_two_fa
      post :disable_two_fa
      delete 'remove_api_key/:id', action: :remove_api_key, as: :remove_api_key
    end

    resource :legendary_badger, only: [:show, :update], path: '/legendary-badger' do
      get '/', action: :show
      patch '/', action: :update
    end

    get '/dashboard', to: 'home#dashboard', as: :dashboard
    get '/terms-and-conditions', to: 'home#terms_and_conditions', as: :terms_and_conditions
    get '/privacy-policy', to: 'home#privacy_policy', as: :privacy_policy
    get '/cookies-policy', to: 'home#cookies_policy', as: :cookies_policy
    get '/contact', to: 'home#contact', as: :contact
    get '/about', to: 'home#about', as: :about
    get '/referral-program', to: 'home#referral_program', as: :referral_program
    get '/ref/:code', to: 'ref_codes#apply_code'
    post '/h/:webhook', to: 'api/bots#webhook', as: :webhooks

    get '/portfolio-analyzer', to: 'portfolios#show' # in the future, use only portfolios which can be automated etc.
    resources :portfolios, except: [:index], path: '/portfolio-analyzer' do
      resources :assets, only: [:new, :create, :destroy, :update]
      patch :toggle_smart_allocation
      patch :update_risk_level
      patch :update_benchmark
      patch :update_strategy
      patch :update_backtest_start_date
      patch :update_risk_free_rate
      post :normalize_allocations
      post :duplicate
    end
  end

  get '/cryptocurrency-dollar-cost-averaging', to: redirect("/#{I18n.default_locale}/cryptocurrency-dollar-cost-averaging")
  get '/terms-and-conditions', to: redirect("/#{I18n.default_locale}/terms-and-conditions")
  get '/privacy-policy', to: redirect("/#{I18n.default_locale}/privacy-policy")
  get '/cookies-policy', to: redirect("/#{I18n.default_locale}/cookies-policy")
  get '/referral-program', to: redirect("/#{I18n.default_locale}/referral-program")
  get '/', to: redirect("/#{I18n.default_locale}")
  get '/legendary-badger', to: redirect("/#{I18n.default_locale}/legendary-badger")

  get '/thank-you', to: 'home#confirm_registration', as: :confirm_registration
  get '/sitemap', to: 'sitemap#index', defaults: {format: 'xml'}
  get '/metrics', to: 'metrics#index', as: :bot_btc_metrics
  get '/health-check', to: 'health_check#index', as: :health_check

  get '/ref/:code', to: 'ref_codes#apply_code', as: 'ref_code'
  post '/ref/accept', to: 'ref_codes#accept'

  post '/create-payment-intent', to: 'upgrade#create_stripe_payment_intent'
  post '/update-payment-intent', to: 'upgrade#update_stripe_payment_intent'
  post '/confirm-card-payment', to: 'upgrade#confirm_stripe_payment'

  # get '*path', to: redirect("/#{I18n.default_locale}")

  telegram_webhook TelegramWebhooksController
end
