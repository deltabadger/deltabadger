require 'sidekiq/web'
require 'sidekiq/cron/web'
require 'telegram/bot'
require 'sidekiq/prometheus/exporter'

Rails.application.routes.draw do

  get 'sso', to: 'sso#sso'

  match "/404", to: "errors#redirect_to_root", via: :all
  match "/422", to: "errors#unprocessable_entity", via: :all
  match "/500", to: "errors#internal_server_error", via: :all

  mount ActionCable.server => '/cable'
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
    end
    resources :api_keys, except: [:edit, :update]
    namespace :bots do
      resources :basics
      resources :withdrawals
      resources :webhooks
      resources :barbells
    end
    resources :conversion_rates
    resources :exchanges
    resources :transactions
    resources :subscriptions
    resources :subscription_plans
    resources :subscription_plan_variants
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

  scope "(:locale)", locale: /#{I18n.available_locales.join("|")}/ do
    root to: 'home#index'

    devise_for :users, controllers: { sessions: 'users/sessions', passwords: 'users/passwords', confirmations: 'users/confirmations', registrations: 'users/registrations' }, path: '', path_names: { sign_in: 'login', sign_out: 'logout', sign_up: 'signup' }, skip: [:registrations]
    devise_scope :user do
      get  'signup', to: 'users/registrations#new', as: 'new_user_registration'
      post 'signup', to: 'users/registrations#create', as: 'user_registration'
      get 'verify_two_factor', to: 'users/sessions#verify_two_factor'
      post 'verify_two_factor', to: 'users/sessions#verify_two_factor'
    end

    resource :affiliate, path: 'referral-program', only: [:new, :show] do
      get ':token/confirm_btc_address', action: 'confirm_btc_address', as: :confirm_btc_address
      patch :update_visible_info
      patch :update_btc_address
      # patch :new, to: 'affiliates#new'
      patch :create, to: 'affiliates#create'
    end

    namespace :upgrade do
      get '/', action: :index
      post '/', action: :create_payment
      post :btcpay_payment_ipn
      get :zen_payment_failure
      post :zen_payment_ipn
      get :success
    end

    namespace :settings do
      get '/', action: :index
      patch :hide_welcome_banner
      patch :hide_news_banner
      patch :hide_referral_banner
      patch :update_password
      patch :update_email
      patch :update_name
      get :edit_two_fa
      patch :update_two_fa
      delete 'remove_api_key/:id', action: :remove_api_key, as: :remove_api_key
    end

    resources :barbell_bots, path: "/barbell-bots" do
      get :asset_search, on: :member
      get :new_api_key, on: :member
      post :create_api_key, on: :member
      post :start, on: :member
      post :stop, on: :member
      post :show, on: :member
      get :confirm_restart, on: :member
      get :new_bot_type, on: :collection
    end

    resources :bots, only: [:show]

    get '/dashboard', to: 'home#dashboard', as: :dashboard
    get '/dashboard/bots/:id', to: 'bots#show', as: :dashboard_bot

    get '/calculator', to: 'calculator#show', as: :calculator

    resource :legendary, only: [:show, :update], path: '/legendary-badger' do
      get :show, on: :collection
    end

    get '/terms-and-conditions', to: 'home#terms_and_conditions', as: :terms_and_conditions
    get '/privacy-policy', to: 'home#privacy_policy', as: :privacy_policy
    get '/cookies-policy', to: 'home#cookies_policy', as: :cookies_policy
    get '/contact', to: 'home#contact', as: :contact
    get '/about', to: 'home#about', as: :about
    get '/referral-program', to: 'home#referral_program', as: :referral_program
    get '/ref/:code', to: 'ref_codes#apply_code'
    post '/h/:webhook', to: 'api/bots#webhook', as: :webhooks

    resources :portfolios, except: [:index], path: '/portfolio-analyzer' do
      resources :assets, only: [:new, :create, :destroy, :update]
      get :show, on: :collection
      patch :toggle_smart_allocation
      patch :update_risk_level
      patch :update_benchmark
      patch :update_strategy
      patch :update_backtest_start_date
      patch :update_risk_free_rate
      patch :update_compare_to
      post :normalize_allocations
      post :duplicate
      get :openai_insights
      get :compare
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

  # get '*path', to: redirect("/#{I18n.default_locale}")

  telegram_webhook TelegramWebhooksController
end
