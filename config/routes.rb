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
      resources :dca_dual_assets
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

  devise_for :users, only: :omniauth_callbacks, controllers: { omniauth_callbacks: 'users/omniauth_callbacks' }

  scope "(:locale)", locale: /#{I18n.available_locales.join("|")}/ do
    root to: 'home#index'

    devise_for :users, controllers: { sessions: 'users/sessions', passwords: 'users/passwords', confirmations: 'users/confirmations', registrations: 'users/registrations' }, path: '', path_names: { sign_in: 'login', sign_out: 'logout', sign_up: 'signup' }, skip: [:registrations, :omniauth_callbacks]
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

    resource :upgrade, only: [:show]
    namespace :upgrades do
      resource :instructions, only: [:show]
    end

    namespace :payments do
      resource :btcpay, only: [:create]
      namespace :btcpay do
        resource :ipn, only: [:create]
        resource :success, only: [:show]
      end
      resource :wire, only: [:create]
      resource :zen, only: [:create]
      namespace :zen do
        resource :ipn, only: [:create]
        resource :success, only: [:show]
        resource :failure, only: [:show]
      end
    end

    namespace :settings do
      get '/', action: :index
      patch :hide_welcome_banner
      patch :hide_news_banner
      patch :hide_referral_banner
      patch :update_password
      patch :update_email
      patch :update_name
      patch :update_time_zone
      get :edit_two_fa
      patch :update_two_fa
      get 'confirm_destroy_api_key/:id', action: :confirm_destroy_api_key, as: :confirm_destroy_api_key
      delete 'destroy_api_key/:id', action: :destroy_api_key, as: :destroy_api_key
      get :community_access_instructions
    end

    get :dashboard, to: redirect { |params, request|
      "/#{request.params[:locale] || I18n.default_locale}/bots"
    }

    namespace :bots do
      resources :dca_single_assets, only: [:create]
      namespace :dca_single_assets do
        resource :pick_buyable_asset, only: [:new, :create]
        resource :pick_exchange, only: [:new, :create]
        resource :add_api_key, only: [:new, :create]
        resource :pick_spendable_asset, only: [:new, :create]
        resource :confirm_settings, only: [:new, :create]
      end
      resources :dca_dual_assets, only: [:create]
      namespace :dca_dual_assets do
        resource :pick_first_buyable_asset, only: [:new, :create]
        resource :pick_second_buyable_asset, only: [:new, :create]
        resource :pick_exchange, only: [:new, :create]
        resource :add_api_key, only: [:new, :create]
        resource :pick_spendable_asset, only: [:new, :create]
        resource :confirm_settings, only: [:new, :create]
      end
    end

    resources :bots do
      resource :start, only: [:edit, :update], controller: 'bots/starts'
      resource :stop, only: [:update], controller: 'bots/stops'
      resource :delete, only: [:edit, :destroy], controller: 'bots/deletes'
      resource :add_api_key, only: [:new, :create], controller: 'bots/add_api_keys'
      resource :asset_search, only: [:edit], controller: 'bots/asset_searches'
      resource :export, only: [:create], controller: 'bots/exports'
      resources :transactions, only: [:destroy], controller: 'bots/cancel_orders'
      post :show
      get :show_index_bot, on: :collection # TODO: move to custom :show logic according to bot type
    end

    get '/calculator', to: 'calculator#show', as: :calculator

    resource :legendary, only: [:show, :update], path: '/legendary-badger'

    get '/terms-and-conditions', to: 'home#terms_and_conditions', as: :terms_and_conditions
    get '/privacy-policy', to: 'home#privacy_policy', as: :privacy_policy
    get '/cookies-policy', to: 'home#cookies_policy', as: :cookies_policy
    get '/contact', to: 'home#contact', as: :contact
    get '/about', to: 'home#about', as: :about
    get '/referral-program', to: 'home#referral_program', as: :referral_program
    get '/ref/:code', to: 'ref_codes#apply_code'
    post '/h/:webhook', to: 'api/bots#webhook', as: :webhooks

    resources :portfolios, except: [:index], path: '/portfolio-analyzer' do
      resources :portfolio_assets, only: [:new, :create, :destroy, :update], as: :assets
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
      get :confirm_destroy
    end

    namespace :surveys do
      resource :onboarding, only: [:create] do
        get 'step-one', to: 'onboardings#new_step_one'
        get 'step-two', to: 'onboardings#new_step_two'
      end
    end

    resources :articles, only: [:index, :show]

    namespace :broadcasts do
      post :metrics_update
      post :pnl_update
      post :price_limit_info_update
      post :price_drop_limit_info_update
      post :indicator_limit_info_update
      post :moving_average_limit_info_update
      post :fetch_open_orders
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
