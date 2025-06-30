class Article < ApplicationRecord
  validates :slug, presence: true, uniqueness: { scope: :locale }
  validates :locale, presence: true, inclusion: { in: I18n.available_locales.map(&:to_s) }
  validates :title, presence: true
  validates :content, presence: true

  scope :published, -> { where(published: true) }
  scope :by_locale, ->(locale) { where(locale: locale.to_s) }
  scope :recent, -> { order(published_at: :desc, created_at: :desc) }

  before_save :set_published_at, if: :will_save_change_to_published?
  before_save :calculate_reading_time
  before_save :set_excerpt_if_blank

  belongs_to :author, optional: true

  include Renderable
  include Paywallable

  def to_param
    slug
  end

  def available_locales
    Article.where(slug: slug).pluck(:locale)
  end

  def translated_versions
    Article.where(slug: slug).where.not(locale: locale)
  end

  def published?
    published && published_at.present? && published_at <= Time.current
  end

  def thumbnail_path
    return nil unless thumbnail.present?

    asset_path = "articles/thumbnails/#{thumbnail}"

    # Check if asset exists in pipeline or filesystem
    if Rails.application.assets&.find_asset(asset_path) ||
       File.exist?(Rails.root.join("app/assets/images/#{asset_path}"))
      asset_path
    end
  end

  private

  def set_published_at
    if published?
      self.published_at ||= Time.current
    else
      self.published_at = nil
    end
  end

  def calculate_reading_time
    return unless content.present?

    # Average reading speed: 200-250 words per minute
    words_per_minute = 200
    word_count = content.split.size
    self.reading_time_minutes = (word_count.to_f / words_per_minute).ceil
  end

  def set_excerpt_if_blank
    return if excerpt.present? || content.blank?

    # Extract first paragraph or first 160 characters
    first_paragraph = content.split("\n\n").first
    return unless first_paragraph.present?

    self.excerpt = first_paragraph.strip.truncate(160)
  end
end
