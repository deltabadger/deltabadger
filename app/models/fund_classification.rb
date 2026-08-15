class FundClassification < ApplicationRecord
  belongs_to :user

  enum :kind, { share: 0, fund: 1, other_security: 2 }
  enum :fund_category, { equity_fund: 0, mixed_fund: 1, real_estate_fund: 2,
                         foreign_real_estate_fund: 3, other_fund: 4 }

  validates :symbol, presence: true
  validates :kind, presence: true
  validates :symbol, uniqueness: { scope: :user_id }
  validates :fund_category, presence: true, if: :fund?
  validates :fund_category, absence: true, unless: :fund?

  # Returns the user's persisted classification for this symbol, or an unsaved proposal.
  # Callers tell the three states apart with two questions: `persisted?` (the user has
  # decided) and `kind.nil?` (unclassified — nothing to propose, and the report must block
  # on it). A proposal with a `kind` is valid and ready for the caller to save as-is.
  def self.resolve(user:, symbol:, asset: nil)
    existing = user.fund_classifications.find_by(symbol: symbol)
    return existing if existing

    proposal = case asset&.instrument_type
               when 'etf'
                 # US-domiciled ETFs publish no InvStG-compliant Anlagebedingungen, so `other_fund`
                 # (sonstige Fonds, 0% Teilfreistellung) is the correct default proposal. §20(4) InvStG
                 # lets the investor prove the actual Kapitalbeteiligungsquote in the Veranlagung
                 # (BMF 21.05.2019 Tz 20.11-20.12) — so an override to equity_fund is the user's own
                 # decision and their burden of proof, which is why this is a proposal, not a lookup.
                 { kind: :fund, fund_category: :other_fund }
               when 'stock'
                 { kind: :share }
               else
                 {}
               end

    # Built off the class, NOT `user.fund_classifications.new` — a record built through the
    # association joins its target and is saved along with the parent, so a `user.save`
    # anywhere downstream would try to persist an unconfirmed (and, when unclassified,
    # invalid) proposal behind the user's back.
    new(user: user, symbol: symbol, isin: asset&.isin, **proposal)
  end
end
