require 'administrate/base_dashboard'

class UserDashboard < Administrate::BaseDashboard
  # ATTRIBUTE_TYPES
  # a hash that describes the type of each of the model's fields.
  #
  # Each different type represents an Administrate::Field object,
  # which determines how the attribute is displayed
  # on pages throughout the dashboard.
  ATTRIBUTE_TYPES = {
    api_keys: Field::HasMany.with_options(sort_by: :id, direction: :desc),
    exchanges: Field::HasMany.with_options(sort_by: :id, direction: :desc),
    bots: Field::HasMany.with_options(sort_by: :id, direction: :desc),
    subscriptions: Field::HasMany.with_options(sort_by: :id, direction: :desc),
    subscription: SubscriptionField,
    payments: Field::HasMany.with_options(sort_by: :id, direction: :desc),
    affiliate: Field::HasOne,
    id: Field::Number,
    email: Field::String,
    name: Field::String,
    encrypted_password: Field::String,
    reset_password_token: Field::String,
    reset_password_sent_at: Field::DateTime,
    remember_created_at: Field::DateTime,
    confirmation_token: Field::String,
    confirmed_at: Field::DateTime,
    confirmation_sent_at: Field::DateTime,
    created_at: Field::DateTime,
    updated_at: Field::DateTime,
    unconfirmed_email: Field::String,
    admin: Field::Boolean,
    terms_and_conditions: Field::Boolean,
    updates_agreement: Field::Boolean,
    welcome_banner_dismissed: Field::Boolean,
    news_banner_dismissed: Field::Boolean,
    referral_banner_dismissed: Field::Boolean,
    limit_reached?: Field::Boolean,
    referrer: Field::HasOne.with_options(class_name: 'Affiliate'),
    otp_secret_key: Field::String,
    otp_module: Field::Select.with_options(collection: %w[disabled enabled])
  }.freeze

  # COLLECTION_ATTRIBUTES
  # an array of attributes that will be displayed on the model's index page.
  #
  # By default, it's limited to four items to reduce clutter on index pages.
  # Feel free to add, remove, or rearrange items.
  COLLECTION_ATTRIBUTES = %i[
    id
    email
    name
    subscription
    admin
  ].freeze

  # SHOW_PAGE_ATTRIBUTES
  # an array of attributes that will be displayed on the model's show page.
  SHOW_PAGE_ATTRIBUTES = %i[
    id
    email
    name
    api_keys
    exchanges
    bots
    subscriptions
    payments
    affiliate
    reset_password_sent_at
    remember_created_at
    confirmed_at
    confirmation_sent_at
    created_at
    updated_at
    unconfirmed_email
    otp_module
    admin
    terms_and_conditions
    updates_agreement
    welcome_banner_dismissed
    news_banner_dismissed
    referral_banner_dismissed
    referrer
  ].freeze

  # FORM_ATTRIBUTES
  # an array of attributes that will be displayed
  # on the model's form (`new` and `edit`) pages.
  FORM_ATTRIBUTES = %i[
    email
    name
    confirmed_at
    confirmation_sent_at
    unconfirmed_email
    otp_module
    admin
    terms_and_conditions
    updates_agreement
    welcome_banner_dismissed
    news_banner_dismissed
    referral_banner_dismissed
  ].freeze

  # COLLECTION_FILTERS
  # a hash that defines filters that can be used while searching via the search
  # field of the dashboard.
  #
  # For example to add an option to search for open resources by typing "open:"
  # in the search field:
  #
  #   COLLECTION_FILTERS = {
  #     open: ->(resources) { where(open: true) }
  #   }.freeze
  COLLECTION_FILTERS = {}.freeze

  # Overwrite this method to customize how users are displayed
  # across all pages of the admin dashboard.
  #
  def display_resource(user)
    user.email.to_s
  end
end
