class Author < ApplicationRecord
  validates :name, presence: true
  validates :url, format: { with: URI::DEFAULT_PARSER.make_regexp(%w[http https]), allow_blank: true }

  has_many :articles, dependent: :nullify

  def to_s
    name
  end
end
