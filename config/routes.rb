require 'sidekiq/web'
require 'sidekiq/cron/web'
require 'sidekiq/prometheus/exporter'

Rails.application.routes.draw do
  # Setup wizard for initial admin configuration
  get '/setup', to: 'setup#new', as: :new_setup
  post '/setup', to: 'setup#create', as: :setup

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
    resources :api_keys, except: [:edit, :update]
    namespace :bots do
      resources :basics
      resources :withdrawals
      resources :dca_dual_assets
    end
    resources :exchanges
    resources :transactions

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
  end

  scope "(:locale)", locale: /#{I18n.available_locales.join("|")}/ do
    root to: 'home#index'

    devise_for :users, controllers: { sessions: 'users/sessions', passwords: 'users/passwords', confirmations: 'users/confirmations', registrations: 'users/registrations' }, path: '', path_names: { sign_in: 'login', sign_out: 'logout', sign_up: 'signup' }, skip: [:registrations, :omniauth_callbacks]
    devise_scope :user do
      get  'signup', to: 'users/registrations#new', as: 'new_user_registration'
      post 'signup', to: 'users/registrations#create', as: 'user_registration'
      get 'verify_two_factor', to: 'users/sessions#verify_two_factor'
      post 'verify_two_factor', to: 'users/sessions#verify_two_factor'
    end

    namespace :settings do
      get '/', action: :index
      patch :update_password
      patch :update_email
      patch :update_name
      patch :update_time_zone
      get :edit_two_fa
      patch :update_two_fa
      get 'confirm_destroy_api_key/:id', action: :confirm_destroy_api_key, as: :confirm_destroy_api_key
      delete 'destroy_api_key/:id', action: :destroy_api_key, as: :destroy_api_key
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
  get '/', to: redirect("/#{I18n.default_locale}")

  get '/thank-you', to: 'home#confirm_registration', as: :confirm_registration
  get '/sitemap', to: 'sitemap#index', defaults: {format: 'xml'}
  get '/health-check', to: 'health_check#index', as: :health_check

  # get '*path', to: redirect("/#{I18n.default_locale}")
end
