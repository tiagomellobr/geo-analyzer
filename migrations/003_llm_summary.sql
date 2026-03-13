-- Adiciona coluna de resumo gerado pela LLM para cada página analisada
ALTER TABLE pages ADD COLUMN llm_summary TEXT;
