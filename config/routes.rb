require 'sidekiq/web'
require 'telegram/bot'

Rails.application.routes.draw do
  require 'sidekiq/prometheus/exporter'
  mount Sidekiq::Prometheus::Exporter => '/sidekiq-metrics'

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
  end

  get '/thank-you', to: 'home#confirm_registration', as: :confirm_registration
  get '/sitemap' => 'sitemap#index', :defaults => { :format => 'xml' }
  get '/metrics' => 'metrics#index', as: :bot_btc_metrics
  get '/health_check' => 'health_check#index', as: :health_check

  authenticate :user, lambda { |u| u.admin? } do
    mount Sidekiq::Web => '/sidekiq'
  end

  get '/ref/:code', to: 'ref_codes#apply_code', as: 'ref_code'
  post '/ref/accept', to: 'ref_codes#accept'

  post '/create-payment-intent', to: 'upgrade#create_card_intent'
  post '/update-payment-intent', to: 'upgrade#update_card_intent'
  post '/confirm-card-payment', to: 'upgrade#confirm_card_payment'

  scope "/(:lang)", lang: /#{I18n.available_locales.join("|")}/ do
    namespace :upgrade do
      get '/', action: :index
      post :pay
      get :payment_success
      post :payment_callback
      post :wire_transfer
    end

    namespace :settings do
      get '/', action: :index
      patch :hide_welcome_banner
      patch :hide_referral_banner
      patch :update_password
      patch :update_email
      post :enable_two_fa
      post :disable_two_fa
      delete 'remove_api_key/:id', action: :remove_api_key, as: :remove_api_key
    end

    resource :affiliate, path: 'referral-program', only: [:new, :create, :show] do
      get ':token/confirm_btc_address', action: 'confirm_btc_address', as: :confirm_btc_address
      patch :update_visible_info
      patch :update_btc_address
      patch :new, to: 'affiliates#new'
      patch :create, to: 'affiliates#create'
    end


    devise_for :users, controllers: { sessions: 'users/sessions', passwords: 'users/passwords', confirmations: 'users/confirmations' }, skip: [:registrations]

    as :user do
      scope :users do
        get '/cancel', to: 'users/registrations#cancel', as: 'cancel_user_registration'
        get '/sign_up', to: 'users/registrations#new', as: 'new_user_registration'
        post '/', to: 'users/registrations#create', as: 'user_registration'
      end
    end

    root to: 'home#index'

    get '/dashboard', to: 'home#dashboard', as: :dashboard
    get '/terms-and-conditions', to: 'home#terms_and_conditions', as: :terms_and_conditions
    get '/privacy-policy', to: 'home#privacy_policy', as: :privacy_policy
    get '/cookies-policy', to: 'home#cookies_policy', as: :cookies_policy
    get '/contact', to: 'home#contact', as: :contact
    get '/about', to: 'home#about', as: :about
    get '/referral-program', to: 'home#referral_program', as: :referral_program
    get '/ref/:code', to: 'ref_codes#apply_code'
  end

  get '/cryptocurrency-dollar-cost-averaging' => redirect("/#{I18n.default_locale}/cryptocurrency-dollar-cost-averaging")
  get '/terms_and_conditions' => redirect("/#{I18n.default_locale}/terms-and-conditions")
  get '/privacy_policy' => redirect("/#{I18n.default_locale}/privacy-policy")
  get '/cookies_policy' => redirect("/#{I18n.default_locale}/cookies-policy")
  get '/referral_program' => redirect("/#{I18n.default_locale}/referral-program")
  get '/' => redirect("/#{I18n.default_locale}")
  get '*path' => redirect("/#{I18n.default_locale}")

  telegram_webhook TelegramWebhooksController
end
